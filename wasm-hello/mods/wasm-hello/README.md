# wasm-hello

The smallest WASM mod: a counter, two buttons and a canvas to draw on, running
as sandboxed WebAssembly in a Herdr pane. It's the template for writing your
own.

Show it: `herdr-app do tab.new '{"mod": "wasm-hello"}'`.

**Permissions:** none. It can't read or write files, run programs or use the
network.

**Needs the WASM Mods core:** the first time it's shown on a machine with no
WASM Mods host, the pane offers to install the host pinned in
`host.lock.json`:
- **Download**, checked against its SHA-256;
- or **Build from source** with cargo.

Every WASM mod shares that one host. Add the `wasm-mods` mod too, to see and
change what each WASM mod may do.

Files: `main.js` and `mod.json` (the Herdr mod), `wasm-bridge.js` (the same in
every WASM mod), `hello.wasm` (the module), `host.lock.json` (the host it
needs).
