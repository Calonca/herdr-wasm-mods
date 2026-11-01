# wasm-todo

A list of things to do, kept in a file, running as sandboxed WebAssembly in a
Herdr pane. Type and press Enter to add; Enter or space ticks the selected
item off; × removes it.

Show it: `herdr-app do tab.new '{"mod": "wasm-todo"}'`.

**Permissions**, asked in its pane before it gets them:
- read files in `~/.local/share/wasm-mods/todo`
- change files in `~/.local/share/wasm-mods/todo`

The list is `~/.local/share/wasm-mods/todo/todo.json`. Nothing else is
reachable: no other files, no programs, no network. A new build asks again.

**Needs the WASM Mods core:** the first time it's shown on a machine with no
WASM Mods host, the pane offers to install the host pinned in
`host.lock.json`:
- **Download**, checked against its SHA-256;
- or **Build from source** with cargo.

Every WASM mod shares that one host. Add the `wasm-mods` mod too, to see and
change what each WASM mod may do.

Files: `main.js` and `mod.json` (the Herdr mod), `wasm-bridge.js` (the same in
every WASM mod), `todo.wasm` (the module), `host.lock.json` (the host it
needs).
