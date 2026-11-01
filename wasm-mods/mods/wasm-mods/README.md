# wasm-mods: the WASM Mods core

The screen where you manage WASM mods in Herdr: every installed module, what
it asked for, and your answers. You can deny, ask again, turn a module off or
on, and remove one. It also shows the installed hosts.

Open it with `herdr-app do mod.open '{"name": "wasm-mods"}'`, or bind a key in
`keys.json`: `"global": { "ctrl-alt-w": "mod.open:wasm-mods" }`.

It also runs any installed module by name:
`herdr-app do tab.new '{"mod": "wasm-mods", "props": {"module": "todo"}}'`.

**Permissions:** it asks for none of its own. It runs `wasm-mods-host
--permissions`, `--deny`, `--forget`, `--enable`, `--disable` and `--remove`.
A permission can only be allowed from a module's own prompt, never from
here.

**The host:** WASM mods run in `wasm-mods-host`, a small program shared by
every WASM mod on this machine, in `~/.local/share/wasm-mods/host/<version>/`.
When none is installed, a WASM mod's pane offers to install the version pinned
in its `host.lock.json`:
- **Download** from that URL, installed only if its SHA-256 matches;
- or **Build from source** with cargo.

Nothing is installed until you click.

Linux, x86_64 and aarch64. The aarch64 host hasn't been tried on real
hardware yet.
