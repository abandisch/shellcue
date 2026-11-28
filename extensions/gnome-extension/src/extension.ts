import Clutter from "gi://Clutter";
import Gio from "gi://Gio";
import GLib from "gi://GLib";
import Meta from "gi://Meta";
import St from "gi://St";

import { Extension } from "resource:///org/gnome/shell/extensions/extension.js";
import * as PanelMenu from "resource:///org/gnome/shell/ui/panelMenu.js";

declare let DEBUG: boolean;

const LOG_PREFIX = "ShellCue GNOME Integration:";

/**
 * @param {any[]} messages
 * @returns {void}
 */
function log_msg(...messages) {
  return console.log(LOG_PREFIX, ...messages);
}

/**
 * @param {any[]} messages
 * @returns {void}
 */
function debug(...messages) {
  if (DEBUG) {
    return console.log(LOG_PREFIX, ...messages);
  }
}

const RESOURCES_PREFIX =
  "resource:///org/gnome/shell/extensions/shellcue-gnome-integration";

const INITIAL_BACKOFF_MS = 1000;
const MAX_BACKOFF_MS = 20_000;
const STABLE_CONNECTION_MS = 5000;

/**
 * @template {string[]} T
 * @param  {T} resources
 * @returns {T extends [string, string, ...string[]] ? string[] : string}
 */
function resource(...resources) {
  if (resources.length >= 2) return resources.map((path) => resource(path));
  else if (resources.length === 1) return `${RESOURCES_PREFIX}/${resources[0]}`;
  else if (DEBUG)
    throw new RangeError("Expected one or more resources, got zero");
  else return null;
}

/**
 * Returns the path to the desktop socket.
 *
 * @private
 * @function
 * @returns {string|null} The path to the desktop socket, or null when
 * `XDG_RUNTIME_DIR` is unset or empty.
 */
function socket_address() {
  const runtime_dir = GLib.getenv("XDG_RUNTIME_DIR");
  if (!runtime_dir) return null;
  return `${runtime_dir}/shellcue/desktop.sock`;
}

/**
 * Converts a message to the format that the desktop socket expects.
 *
 * @private
 * @function
 * @param {string} hook The hook that the payload is for.
 * @param {object} payload The payload of the message.
 * @returns {Uint8Array} The converted message.
 */
function socket_encode(hook, payload) {
  const header = "\x1b@fig-json\x00\x00\x00\x00\x00\x00\x00\x00";
  const body = JSON.stringify({ hook: { [hook]: payload } });

  const encoder = new TextEncoder();
  const header_bytes = encoder.encode(header);
  const body_bytes = encoder.encode(body);

  // I'd use a Uint32Array pointing to the same buffer to do this, but the
  // length part of the header is misaligned by two bytes...
  let length = body_bytes.length << 0;
  for (let i = 0; i < 4; i++) {
    const byte = length & 0xff;
    header_bytes[header_bytes.length - i - 1] = byte;
    length = (length - byte) / 256;
  }

  const message = new Uint8Array(header_bytes.length + body_bytes.length);
  message.set(header_bytes);
  message.set(body_bytes, header_bytes.length);
  return message;
}

/**
 * The main class for managing the extensions state.
 */
export default class QCliExtension extends Extension {
  /** @public @property @type {boolean} */
  get connected() {
    return this.#connected;
  }

  /**
   * Whether or not the extension is connected to the desktop socket.
   *
   * @private @property @type {boolean}
   */
  #connected = false;

  /**
   * The binding object between the extension's `connected` property and the
   * panel icon's `connected` property. This enables automatically updating
   * the panel icon's connected state whenever the extension's connected
   * property changes.
   *
   * @private @property @type {import("../types/.gobject").Binding|null}
   */
  #connected_binding = null;

  /**
   * A map of GObject to connection id's. This is used by the extension to keep track
   * of which relevant objects (global display, windows, the cursor tracker) we connect to,
   * and disconnect when the connection to the desktop socket is dropped.
   *
   * @private @property @type {Map<import("../types/.gobject").Object, Set<number>>}
   */
  #connections = new Map();

  /**
   * True between `enable()` and `disable()`; nothing may start, and no late
   * callback may act, once it is false.
   *
   * @private @property @type {boolean}
   */
  #enabled = false;

  /**
   * The widget representing the ShellCue icon in the panel.
   *
   * @private @property @type {PanelIcon|null}
   */
  #panel_icon = null;

  /** @private @property @type {import("../types/.gio").Resource|null} */
  #resources = null;

  /**
   * The extension settings, set according to the schema defined under
   * [../schemas/org.gnome.shell.extensions.shellcue-gnome-integration.gschema.xml]
   *
   * @private @property @type {import("../types/.gio").Settings|null}
   */
  #settings = null;

  /**
   * The connection to the desktop socket; held for as long as `#socket` is
   * used, since a GIOStream closes itself when it is finalised.
   *
   * @private @property @type {import("../types/.gio").SocketConnection|null}
   */
  #connection = null;

  /**
   * Monotonic time, in microseconds, at which the current connection was made.
   *
   * @private @property @type {number}
   */
  #connected_since = 0;

  /**
   * The desktop socket.
   *
   * @private @property @type {import("../types/.gio").Socket|null}
   */
  #socket = null;

  /**
   * The window currently in focus.
   *
   * @private @property @type {import("../types/.gobject").Object|null} */
  #window = null;

  /**
   * The cancellable of the connection attempt in flight, else null.
   *
   * @private @property @type {import("../types/.gio").Cancellable|null}
   */
  #connect_cancellable = null;

  /**
   * The GLib source id of the scheduled retry, 0 when none.
   *
   * @private @property @type {number}
   */
  #retry_source_id = 0;

  /**
   * The delay of the last scheduled retry, in milliseconds; reset to
   * `INITIAL_BACKOFF_MS`, which is multiplied before it is first used.
   *
   * @private @property @type {number}
   */
  #backoff_ms = INITIAL_BACKOFF_MS;

  /**
   * The number of connection attempts in the current connecting run.
   *
   * @private @property @type {number}
   */
  #attempts = 0;

  /**
   * Enables the extension, starting to connect to the desktop socket quietly
   * in the background. The mutter signals are connected once the connection
   * is made.
   *
   * @public @method @returns {void}
   */
  enable() {
    debug("Enabling the extension");

    // Load and register resource files.
    this.#resources = Gio.Resource.load(
      `${this.metadata.path}/resources/shellcue-gnome-integration.gresource`,
    );
    Gio.resources_register(this.#resources);

    // Get the settings object.
    this.#settings = this.getSettings();

    // Watch for the user changing the "show-panel-icon" preference.
    // this.#connect_to_object(this.#settings, "changed::show-panel-icon", () => {
    //   if (this.#settings.get_boolean("show-panel-icon")) {
    //     // If the panel icon doesn't exist, create it and bind the connected
    //     // property, then add it to the panel.
    //     if (this.#panel_icon === null) {
    //       this.#panel_icon = new PanelIcon({
    //         connected: this.#connected,
    //       });
    //       this.#connected_binding = this.bind_property(
    //         "connected",
    //         this.#panel_icon,
    //         "connected",
    //         GObject.BindingFlags.DEFAULT,
    //       );
    //     }
    //     Main.panel.addToStatusArea(
    //       "ShellCue",
    //       this.#panel_icon,
    //       0,
    //       "right",
    //     );
    //     // If the panel icon exists, destroy it.
    //   } else if (this.#panel_icon !== null) {
    //     this.#connected_binding.unbind();
    //     this.#connected_binding = null;
    //     this.#panel_icon.destroy();
    //     this.#panel_icon = null;
    //   }
    // });

    // if (this.#settings.get_boolean("show-panel-icon")) {
    //   this.#panel_icon = new PanelIcon({
    //     connected: this.#connected,
    //   });
    //   this.#connected_binding = this.bind_property(
    //     "connected",
    //     this.#panel_icon,
    //     "connected",
    //     GObject.BindingFlags.DEFAULT,
    //   );
    //   Main.panel.addToStatusArea("ShellCue", this.#panel_icon, 0, "right");
    // }

    this.#enabled = true;
    this.#start_connecting();
  }

  /**
   * Disables the extension. Once this returns, no GLib source, signal
   * connection or held socket of this extension remains; a connect still
   * pending can no longer act, and closes anything it is handed.
   *
   * @public @method @returns {void}
   */
  disable() {
    debug("Disabling the extension");

    this.#enabled = false;
    this.#stop_connecting();
    this.#drop_connection();

    // Unregister the resource files.
    Gio.resources_unregister(this.#resources);
    this.#resources = null;

    // Disconnect from and delete the settings object.
    this.#disconnect_from_object(this.#settings, null);
    this.#settings = null;

    // If the panel icon exists, destroy it.
    if (this.#panel_icon !== null) {
      this.#connected_binding.unbind();
      this.#connected_binding = null;
      this.#panel_icon.destroy();
      this.#panel_icon = null;
    }
  }

  /**
   * Starts a connecting run from `enable()`, unless the extension is
   * disabled, connected, already attempting to connect or waiting to retry.
   *
   * @returns {void}
   */
  #start_connecting() {
    if (
      !this.#enabled ||
      this.#socket !== null ||
      this.#connect_cancellable !== null ||
      this.#retry_source_id !== 0
    )
      return;

    if (socket_address() === null) {
      log_msg(
        "The runtime folder (XDG_RUNTIME_DIR) is unknown; not connecting to the ShellCue app.",
      );
      return;
    }

    this.#attempts = 0;
    this.#backoff_ms = INITIAL_BACKOFF_MS;
    this.#attempt();
  }

  /**
   * Makes one connection attempt to the desktop socket. A callback that
   * arrives after the attempt was cancelled or superseded, or after the
   * extension was disabled, does nothing but close what it was handed.
   *
   * @returns {void}
   */
  #attempt() {
    const socket_path = socket_address();
    const client = Gio.SocketClient.new();
    const cancellable = Gio.Cancellable.new();
    this.#attempts++;

    debug(`Connecting to "${socket_path}" (attempt ${this.#attempts})`);

    try {
      const address = Gio.UnixSocketAddress.new(socket_path);
      this.#connect_cancellable = cancellable;
      client.connect_async(address, cancellable, (_source, result) => {
        const current =
          this.#connect_cancellable === cancellable && this.#enabled;

        let connection;
        try {
          connection = client.connect_finish(result);
        } catch (error) {
          if (!current) return;

          this.#connect_cancellable = null;
          if (this.#attempts === 1)
            log_msg(
              `Could not reach the ShellCue app; will keep trying quietly. (${error})`,
            );
          else debug(`Attempt ${this.#attempts} failed: ${error}`);
          this.#schedule_retry();
          return;
        }

        if (!current) {
          try {
            connection.close(null);
          } catch (error) {
            debug(`Closing a stale connection failed: ${error}`);
          }
          return;
        }

        this.#connect_cancellable = null;
        this.#connection = connection;
        this.#connected_since = GLib.get_monotonic_time();
        this.#socket = connection.get_socket();
        // Blocking sends would stall the compositor on a peer that never reads.
        this.#socket.set_blocking(false);
        this.#connected = true;
        log_msg("Connected to the ShellCue app.");

        this.#connect_to_mutter();
        this.#send_window_data(null);
      });
    } catch (error) {
      // Not retried: the same call would throw again.
      this.#connect_cancellable = null;
      log_msg(`Could not start connecting to the ShellCue app: ${error}`);
    }
  }

  /**
   * Schedules the next connection attempt, backing off by 10% each time up
   * to `MAX_BACKOFF_MS`; the first retry after a reset waits 1.1 times
   * `INITIAL_BACKOFF_MS`.
   *
   * @returns {void}
   */
  #schedule_retry() {
    if (!this.#enabled || this.#retry_source_id !== 0) return;

    this.#backoff_ms = Math.min(this.#backoff_ms * 1.1, MAX_BACKOFF_MS);
    this.#retry_source_id = GLib.timeout_add(
      GLib.PRIORITY_LOW,
      Math.round(this.#backoff_ms),
      () => {
        this.#retry_source_id = 0;
        if (this.#enabled) this.#attempt();
        return GLib.SOURCE_REMOVE;
      },
    );
  }

  /**
   * Cancels the connection attempt in flight and the scheduled retry, if any.
   *
   * @returns {void}
   */
  #stop_connecting() {
    if (this.#connect_cancellable !== null) {
      const cancellable = this.#connect_cancellable;
      // Cleared first: the callback must see itself as superseded.
      this.#connect_cancellable = null;
      cancellable.cancel();
    }

    if (this.#retry_source_id !== 0) {
      GLib.Source.remove(this.#retry_source_id);
      this.#retry_source_id = 0;
    }
  }

  /**
   * Removes every signal connection and closes the connection to the desktop
   * socket. Safe to call when nothing is connected.
   *
   * @returns {void}
   */
  #drop_connection() {
    for (const [object, connections] of this.#connections) {
      for (const connection of connections) object.disconnect(connection);
      connections.clear();
    }
    this.#connections.clear();
    this.#window = null;

    if (this.#connection !== null) {
      try {
        this.#connection.close(null);
      } catch (error) {
        debug(`Closing the connection failed: ${error}`);
      }
    }
    this.#connection = null;
    this.#socket = null;
    this.#connected = false;
  }

  /**
   * Connects to all of the signals that this extension uses from mutter.
   *
   * The connection ids are stored in a map so that they may be disconnected
   * later to ensure garbage collection.
   *
   * @returns {void}
   */
  #connect_to_mutter() {
    debug("Connecting to mutter");

    this.#window = global.display.focus_window;

    this.#connect_to_object(this.#window, "size-changed", () => {
      debug("mutter: size-changed");
      return this.#send_window_data(null);
    });

    // Subscribe to receive updates when the global `MetaDisplay` "focus-window"
    // property changes.
    this.#connect_to_object(global.display, "notify::focus-window", () => {
      debug(
        `mutter: notify::focus-window on ${global.display.focus_window?.get_wm_class() ?? "nothing"}`,
      );
      if (this.#window !== global.display.focus_window) {
        this.#disconnect_from_object(this.#window, null);

        this.#window = global.display.focus_window;

        this.#connect_to_object(this.#window, "size-changed", () =>
          this.#send_window_data(null),
        );

        this.#send_window_data(null);
      }
    });

    // Subscribe to receive updates when the overlay key is pressed
    this.#connect_to_object(global.display, "overlay-key", () => {
      debug("mutter: overlay-key");
      this.#send_window_data(true);
    });

    // Subscribe to be notified when a new grab operation begins.
    // This is needed because neither GNOME shell or mutter expose a signal that
    // is fired when a `MetaWindow` is moved. So, the solution is to subscribe
    // to the display when a grab operation starts; AKA when the user starts
    // moving around a window, and then updating the window data whenever the
    // cursor moves until the grab operation ends.
    this.#connect_to_object(
      global.display,
      "grab-op-begin",
      (_, __, grab_op) => {
        if (
          grab_op === Meta.GrabOp.MOVING ||
          grab_op === Meta.GrabOp.KEYBOARD_MOVING
        ) {
          if (this.#window !== global.display.focus_window) {
            this.#disconnect_from_object(this.#window, null);

            this.#window = global.display.focus_window;

            this.#connect_to_object(this.#window, "size-changed", () =>
              this.#send_window_data(null),
            );
          }

          this.#send_window_data(null);
          // A failed send drops every connection; adding more would leak them.
          if (this.#socket === null) return;

          const cursor = Meta.CursorTracker.get_for_display(global.display);
          const cursor_connection = this.#connect_to_object(
            cursor,
            "position-invalidated",
            () => this.#send_window_data(null),
          );

          const display_connection = this.#connect_to_object(
            global.display,
            "grab-op-end",
            () => {
              this.#disconnect_from_object(global.display, display_connection);
              this.#disconnect_from_object(cursor, cursor_connection);
            },
          );
        }
      },
    );
  }

  /**
   * Connects to `signal` on `object`, storing a relation between the two so
   * that it can be disconnected automatically if the extension is disabled.
   *
   * @param {import("../types/.gobject").Object} object
   * @param {string} signal
   * @param {() => void} handler
   * @returns {number}
   */
  #connect_to_object(object, signal, handler) {
    if (object === null) return;
    const connections = this.#connections.get(object) ?? new Set();
    const connection = object.connect(signal, handler);
    connections.add(connection);
    this.#connections.set(object, connections);
    return connection;
  }

  /**
   * @param {import("../types/.gobject").Object} object
   * @param {number?} connection
   * @returns {boolean}
   */
  #disconnect_from_object(object, connection) {
    if (object === null) return;

    const connections = this.#connections.get(object) ?? new Set();

    if (connection !== null) {
      object.disconnect(connection);

      const removed = connections.delete(connection);

      // We're not adding connections, so the only change in size could be
      // negative. As such, we only need to check if the set is now empty and
      // delete it if it is to ensure garbage collection.
      if (connections.size === 0) this.#connections.delete(object);

      return removed;
    } else {
      for (const to_disconnect of connections) object.disconnect(to_disconnect);
      return this.#connections.delete(object);
    }
  }

  /**
   * Sends the focused window's data to the desktop socket, or asks the app to
   * hide the popup when no window is focused. A send error or a partial
   * write is handled by `#connection_lost`.
   *
   * @param {boolean} overlay_pressed
   */
  #send_window_data(overlay_pressed) {
    if (this.#socket === null) return;

    let payload;
    if (this.#window === null) {
      const zeros = { x: 0, y: 0, width: 0, height: 0 };
      payload = {
        source: "gse",
        id: "",
        inner: zeros,
        outer: zeros,
        hide: true,
        scale: 1,
      };
    } else {
      // Mutter populates wm class/instance with the app_id on Wayland.
      const wm_class = this.#window.get_wm_class();
      // https://mutter.gnome.org/meta/method.Window.get_frame_rect.html
      const inner_rect = this.#window.get_frame_rect();
      // https://mutter.gnome.org/meta/method.Window.get_buffer_rect.html
      const outer_rect = this.#window.get_buffer_rect();
      const scale = global.display.get_monitor_scale(
        this.#window.get_monitor(),
      );

      debug(
        `Sending data for rect inner ${inner_rect.x},${inner_rect.y},${inner_rect.width},${inner_rect.height}` +
          ` outer ${outer_rect.x},${outer_rect.y},${outer_rect.width},${outer_rect.height} with scale ${scale}`,
      );

      payload = {
        source: "gse",
        id: wm_class,
        inner: {
          x: inner_rect.x,
          y: inner_rect.y,
          width: inner_rect.width,
          height: inner_rect.height,
        },
        outer: {
          x: outer_rect.x,
          y: outer_rect.y,
          width: outer_rect.width,
          height: outer_rect.height,
        },
        hide: overlay_pressed,
        scale,
      };
    }

    const message = socket_encode("focusedWindowData", payload);
    let sent;
    try {
      sent = this.#socket.send(message, null);
    } catch (error) {
      this.#connection_lost(error);
      return;
    }
    // A partial write would break the framing of every later message.
    if (sent !== message.length)
      this.#connection_lost(`wrote ${sent} of ${message.length} bytes`);
  }

  /**
   * Drops a connection that failed and schedules a retry; the backoff is
   * reset only if the connection lasted at least `STABLE_CONNECTION_MS`.
   *
   * @param {any} reason
   * @returns {void}
   */
  #connection_lost(reason) {
    log_msg("Lost the connection to the ShellCue app.");
    debug(`Send failed: ${reason}`);

    const stable =
      GLib.get_monotonic_time() - this.#connected_since >=
      STABLE_CONNECTION_MS * 1000;
    this.#drop_connection();
    if (stable) {
      this.#attempts = 0;
      this.#backoff_ms = INITIAL_BACKOFF_MS;
    }
    this.#schedule_retry();
  }
}

/** @public @class PanelIcon */
// eslint-disable-next-line @typescript-eslint/no-unused-vars
class PanelIcon extends PanelMenu.Button {
  /** @public @property @type {boolean} */
  get connected() {
    return this.#connected;
  }

  /** @public @property @type {boolean} */
  set connected(value) {
    this.#connected = value;
    this.notify("connected");
  }

  /** @private @property @type {boolean} */
  #connected;
  /** @private @property @type {number} */
  #connection;
  /** @private @property @type {import("../types/.st").Icon} */
  #icon;
  /** @private @property @type {import("../types/.gio").Icon} */
  #icon_connected;
  /** @private @property @type {import("../types/.gio").Icon} */
  #icon_disconnected;

  /** @override @method @returns {void} */
  constructor({ connected }) {
    super(0.0, null, true);

    this.#connected = connected;

    const [icon_connected, icon_disconnected] = resource(
      "icons/scalable/actions/shellcue-connected.svg",
      "icons/scalable/actions/shellcue-disconnected.svg",
    );

    this.#icon_connected = Gio.Icon.new_for_string(icon_connected);
    this.#icon_disconnected = Gio.Icon.new_for_string(icon_disconnected);

    this.#icon = new St.Icon({
      gicon: this.#connected ? this.#icon_connected : this.#icon_disconnected,
      style_class: "system-status-icon",
      reactive: true,
      track_hover: true,
      visible: !connected,
    });

    this.add_child(this.#icon);

    this.#connection = this.connect("notify::connected", () => {
      if (this.#connected) {
        this.#icon.gicon = this.#icon_connected;
        this.#icon.visible = false;
      } else {
        this.#icon.gicon = this.#icon_disconnected;
        this.#icon.visible = true;
      }
    });
  }

  /** @override @method @returns {void} */
  vfunc_finalize() {
    this.disconnect(this.#connection);

    this.#connected = null;
    this.#icon = null;
    this.#icon_connected = null;
    this.#icon_disconnected = null;

    super.vfunc_finalize();
  }

  /** @override @method @param {import("../types/.clutter").Event} event @returns {boolean} */
  vfunc_event(event) {
    if (this.menu && event.type() === Clutter.EventType.BUTTON_PRESS)
      Gio.Subprocess.new(["shellcue"], Gio.SubprocessFlags.NONE);

    return Clutter.EVENT_PROPAGATE;
  }
}

/**
 * Initializes the extension, without enabling it, and returns it. This function
 * is expected to be in the toplevel of every extension.
 *
 * @returns {Extension}
 */
// function init() {
//   debug("Initializing ShellCue Extension");
//   var ExtensionClass = GObject.registerClass(
//     {
//       GTypeName: "ShellCueExtension",
//       Properties: {
//         connected: GObject.ParamSpec.boolean(
//           "connected",
//           "connected",
//           "connected",
//           GObject.ParamFlags.READWRITE,
//           false,
//         ),
//       },
//     },
//     QCliExtension,
//   );//

//   GObject.registerClass(
//     {
//       GTypeName: "ShellCuePanelIcon",
//       Properties: {
//         connected: GObject.ParamSpec.boolean(
//           "connected",
//           "connected",
//           "connected",
//           GObject.ParamFlags.READWRITE,
//           false,
//         ),
//       },
//     },
//     PanelIcon,
//   );//

//   return new ExtensionClass();
// }
