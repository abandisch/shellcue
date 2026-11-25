"use strict";

(function () {
  /* The app accepts only fixed tokens; anything else is dropped on its side. */
  function send(command) {
    if (window.ipc && typeof window.ipc.postMessage === "function") {
      window.ipc.postMessage(command);
    }
  }

  function byId(id) {
    return document.getElementById(id);
  }

  function setText(id, text) {
    byId(id).textContent = text;
  }

  function show(id, visible) {
    byId(id).hidden = !visible;
  }

  function setMark(id, mark) {
    byId(id).dataset.mark = mark;
  }

  var current = null;

  function renderShell(shell, busy) {
    setText("shell-scripts-dir", shell.scripts_dir);

    var list = byId("shell-files");
    list.replaceChildren();
    shell.files.forEach(function (file) {
      var item = document.createElement("li");
      item.textContent = file;
      list.appendChild(item);
    });

    var done = shell.status === "done";
    var failed = shell.status === "failed";
    var working = shell.status === "working";
    show("shell-detail", !done);
    show("shell-files", shell.files.length > 0 && !done);
    show("shell-no-files", shell.files.length === 0 && !done);
    show("shell-button", !done);
    byId("shell-button").disabled = busy;
    show("shell-failure", failed);

    if (done) {
      setMark("shell", "done");
      setText("shell-status", "The shell integration is set up. Open a new terminal window so it loads.");
    } else if (working) {
      setMark("shell", "busy");
      setText("shell-status", "Setting up the shell integration…");
    } else if (failed) {
      setMark("shell", "warn");
      setText("shell-status", "");
      setText("shell-error", "The shell integration could not be set up: " + (shell.error || "unknown error"));
      setText("shell-command", shell.command);
    } else {
      setMark("shell", "idle");
      setText("shell-status", "");
    }
  }

  function renderAccessibility(accessibility, busy) {
    var status = accessibility.status;
    var granted = status === "granted";
    show("accessibility-detail", !granted);
    show("accessibility-button", !granted);
    byId("accessibility-button").disabled = busy;

    if (granted) {
      setMark("accessibility", "done");
      setText("accessibility-status", "ShellCue has the Accessibility permission.");
    } else if (status === "waiting") {
      setMark("accessibility", "busy");
      setText(
        "accessibility-status",
        "Waiting for the permission… If System Settings did not open, open it and look under Privacy & Security for Accessibility (named Device Control and Data Access on newer macOS)."
      );
    } else if (status === "timed_out") {
      setMark("accessibility", "warn");
      setText(
        "accessibility-status",
        "Not granted yet. Switch ShellCue on under System Settings > Privacy & Security > Accessibility (named Device Control and Data Access on newer macOS), then come back to this window."
      );
    } else {
      setMark("accessibility", "idle");
      setText("accessibility-status", "");
    }
  }

  function renderInputMethod(inputMethod, busy) {
    var status = inputMethod.status;
    var enabled = status === "enabled";
    setText("input-method-directory", inputMethod.directory);
    show("input-method-detail", !enabled);
    show("input-method-button", !enabled);
    byId("input-method-button").disabled = busy;
    show("input-method-failure", status === "failed");

    if (enabled) {
      setMark("input-method", "done");
      setText("input-method-status", "The input method link is in place. Restart any terminal that uses it.");
    } else if (status === "working") {
      setMark("input-method", "busy");
      setText("input-method-status", "Enabling the input method…");
    } else if (status === "failed") {
      setMark("input-method", "warn");
      setText("input-method-status", "");
      setText("input-method-error", "The input method could not be enabled: " + (inputMethod.error || "unknown error"));
    } else {
      setMark("input-method", "idle");
      setText("input-method-status", "");
    }
  }

  function renderFooter(view) {
    var ready = view.shell.status === "done" && view.accessibility.status === "granted";
    show("footer-pending", !ready);
    show("footer-ready", ready);
    setText(
      "running-note",
      view.menu_bar_icon_hidden
        ? "ShellCue is running. Its menu bar icon is hidden by the app.hideMenubarIcon setting."
        : "ShellCue is running. Look for its icon in the menu bar."
    );
  }

  function reveal(id, previousStatus, status, problem) {
    if (status === problem && previousStatus !== problem) {
      byId(id).scrollIntoView({ block: "nearest" });
    }
  }

  function update(view) {
    var previous = current;
    current = view;
    document.body.classList.remove("loading");
    renderShell(view.shell, view.busy);
    renderAccessibility(view.accessibility, view.busy);
    renderInputMethod(view.input_method, view.busy);
    renderFooter(view);
    reveal("shell", previous && previous.shell.status, view.shell.status, "failed");
    reveal("accessibility", previous && previous.accessibility.status, view.accessibility.status, "timed_out");
    reveal("input-method", previous && previous.input_method.status, view.input_method.status, "failed");
  }

  window.shellcueSetup = Object.freeze({ update: update });

  byId("shell-button").addEventListener("click", function () {
    send("set-up-shell");
  });
  byId("accessibility-button").addEventListener("click", function () {
    send("request-accessibility");
  });
  byId("input-method-button").addEventListener("click", function () {
    send("enable-input-method");
  });
  byId("not-now-button").addEventListener("click", function () {
    send("not-now");
  });
  byId("done-button").addEventListener("click", function () {
    send("close");
  });

  document.addEventListener("keydown", function (event) {
    if (event.key === "Escape" && !(current && current.busy)) {
      send("close");
    }
  });

  document.addEventListener("contextmenu", function (event) {
    if (!event.target.closest(".path, .command, .error, .files li")) {
      event.preventDefault();
    }
  });

  send("ready");
})();
