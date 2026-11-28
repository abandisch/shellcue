# Quirks of GNOME Shell Extensions

GNOME Shell Extensions are directly loaded into the same JavaScript Realm as the
Shell itself, so they have access to the same global variables as the Shell.

Since GNOME 45 an extension is an ES module. It imports libraries with `gi://`
paths (for example `gi://Gio`) and the Shell's own code with `resource://` paths
(for example `resource:///org/gnome/shell/extensions/extension.js`), and it
exports one default class that extends `Extension`. `src/extension.ts` shows
both.

## Prerequisites

```sh
sudo apt install libgirepository1.0-dev
```

## Testing Changes

See <https://gjs.guide/extensions/development/debugging.html#reloading-extensions>

# Quirk (bug) of Node

The build scripts for this repo spawn a ton of promises to do work in
parallel, and for whatever reason, this causes node to have fatal errors when
exceptions are raised in some promises. So, if you get a fatal exception, it's
probably this.
