# wasm-git-graph

The tab's git repo as a commit graph, running as sandboxed WebAssembly in a
Herdr pane: each commit a row with its lanes, hash, branches and tags, subject,
author and age. Enter shows a commit's details (its whole message, who and
when, the files it changed); Space opens its diff in a new tab.

Show it: `herdr-app do tab.new '{"mod": "wasm-git-graph"}'`, in a tab whose
folder is a git repo. Space needs the `wasm-diff` mod too.

**Permissions**, asked in its pane before it gets them:
- know the folder and git branch of the tab it's in
- read the git history and changes of that folder
- read files in `~/.local/share/wasm-mods/git-graph`
- change files in `~/.local/share/wasm-mods/git-graph`

The host runs git for it in the tab's folder, reading commands only (`log`,
`show`, `rev-parse`): nothing is written to the repo, no other program runs,
nothing reaches the network. Whether the details show is kept for the next graph
in `~/.local/share/wasm-mods/git-graph/settings.json`; without the last two
permissions each graph starts with them hidden. A new build asks again.

**Needs the WASM Mods host:** the first time it's shown on a machine with
no host that runs it, the pane offers to install the one pinned in
`host.lock.json`:
- **Download**, checked against its SHA-256;
- or **Build from source** with cargo.

Files: `main.js` and `mod.json` (the Herdr mod), `wasm-bridge.js` (the same in
every WASM mod), `git_graph.wasm` (the module), `host.lock.json` (the host it
needs).
