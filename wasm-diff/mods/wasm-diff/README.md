# wasm-diff

What changed in the tab's git repo, running as sandboxed WebAssembly in a
Herdr pane: uncommitted changes, unpushed commits, the whole branch, or one
commit. The changed files as a tree on the left, the selected file's diff on
the right (or every file's, one after another), unified or side by side, with
syntax colours and the changed words marked.

Show it: `herdr-app do tab.new '{"mod": "wasm-diff"}'`, in a tab whose folder
is a git repo. **commit ▾** shows one of the branch's recent commits.

**Permissions**, asked in its pane before it gets them:
- know the folder and git branch of the tab it's in
- read the git history and changes of that folder
- read files in `~/.local/share/wasm-mods/diff`
- change files in `~/.local/share/wasm-mods/diff`

The host runs git for it in the tab's folder, reading commands only (`diff`,
`status`, `log`, …): nothing is written to the repo, no other program runs, nothing
reaches the network. The view toggles (all files, side by side, wrap) are kept
for the next diff in `~/.local/share/wasm-mods/diff/settings.json`; without the
last two permissions each diff starts with them off. Untracked files are listed but not shown. A new build asks again.

**Needs WASM Mods host 0.2.0:** the first time it's shown on a machine with
no host that runs it, the pane offers to install the one pinned in
`host.lock.json`:
- **Download**, checked against its SHA-256;
- or **Build from source** with cargo.

Files: `main.js` and `mod.json` (the Herdr mod), `wasm-bridge.js` (the same in
every WASM mod), `diff.wasm` (the module), `host.lock.json` (the host it
needs).
