# WASM Mods for Herdr

Sandboxed WebAssembly mods for the Herdr app (herdr-app), with permissions.

> **Experimental.** This is an early release. On Herdr, WASM mods are drawn
> through a JavaScript bridge, which makes them slower than they should be,
> especially with many WASM panes open. A native version that draws them
> directly in Herdr is in development; a link will follow here.

A WASM mod is a small app compiled to WebAssembly that shows in a Herdr pane. It
can't touch your files, run programs or reach the network on its own: it has no
way to. It only describes what to draw, and asks the host for anything else. The
host does that only when you allowed it, per mod and per thing asked.

This is an unofficial add-on. It is not affiliated with or endorsed by Herdr. It
uses what herdr-app offers to every mod and doesn't change the app. Made for
herdr-app 0.1.11, on Linux.

## Permissions

Each module says what it needs in a manifest built into its `.wasm` file:

```rust
manifest!(description: "a list of things to do, kept in a file",
          capabilities: ["fs.read:~/.local/share/wasm-mods/todo",
                         "fs.write:~/.local/share/wasm-mods/todo"]);
```

The first time it's shown, the module's pane asks you, in plain words, before the
module gets any of it:

> **wasm-todo asks to:**
> • read files in ~/.local/share/wasm-mods/todo
> • change files in ~/.local/share/wasm-mods/todo
>
> [Allow] [Deny]

What a module can ask for:

| Capability | What it allows |
|---|---|
| `fs.read:<folder>` | read, list and stat files inside that folder |
| `fs.write:<folder>` | write, make folders and delete inside that folder |
| `window.folder` | know the folder and git branch of the tab it's shown in |
| `git` | run git in that folder, read-only commands only |
| `glab` | read that repo's GitLab projects with `glab api projects/…`, GET only |

### The git permission

`git` lets a module read a repo's history and changes, nothing more. It comes
with `window.folder`: git runs only in the folder of the tab the module is
shown in, and the prompt asks for both:

> **wasm-diff asks to:**
> • know the folder and git branch of the tab it's in
> • read the git history and changes of that folder

The host runs git for it, with no shell, and only reading subcommands (`log`,
`show`, `diff`, `status`, `rev-parse`, …). Options that would write a file, run
another program or read outside the repo are refused, and external diff
drivers, text conversion, fsmonitor and the pager are turned off whatever your
git config says. A module can't run any other program but `glab`, below.

### The glab permission

`glab` lets a module read GitLab for the repo in the tab's folder (its issues,
merge requests, pipelines, …) through your own `glab` login. Like `git` it runs
only in that folder, so it comes with `window.folder`. The host runs only
`glab api <path>`, with no shell: a GET, with no method, fields or other
options, and only for paths under `projects/` (none that climb out with `..`).
Anything else is refused, and the module is told why. Prompts and the pager
are off. It reads whatever your login can read under `projects/`.

- **Nothing is granted without asking.** A module without a manifest gets
  nothing. A capability it uses without declaring it is refused. One this host
  doesn't know is never granted and never asked.
- **Answers are per build.** An answer holds for the exact build you allowed
  (its SHA-256). A rebuilt or swapped `.wasm` file asks again.
- **Capabilities are allowed only from the module's own prompt**, in its own
  pane, so another mod can't allow them on its behalf.
- **Folders are checked, not trusted.** A path is resolved by name, then reached
  by walking down from the allowed folder one component at a time with
  `openat(O_NOFOLLOW)`. A symlink below the allowed folder refuses the request,
  and swapping folders mid-way can't redirect it. Writes go to a new temporary
  file and are renamed into place. A folder that reaches far (your home, `/`,
  `~/.ssh`, `~/.config`) is flagged in the prompt. Limits: a hard link inside an
  allowed folder to a file outside it is readable, as on any Unix; a bind mount
  is followed like a folder.
- **Files you pick are yours to give.** A module may open the desktop's own file
  chooser (the XDG portal on Linux, the system's on macOS); what you pick is
  readable for that session only.
- **Limits on every call.** A module gets 500 ms and 256 MB per call; past
  that it stops.

The answers live in `~/.config/wasm-mods/permissions.toml`. The core mod,
**WASM Mods**, lists every installed module with what it asked for and your
answers. You can deny, forget an answer (it asks again), turn a module off or
on, and remove one. It also shows the installed hosts. Open it with:

```sh
herdr-app do mod.open '{"name": "wasm-mods"}'
```

or bind a key in herdr-app's `keys.json`:
`"global": { "ctrl-alt-w": "mod.open:wasm-mods" }`.

## Install

Each WASM mod is a herdr-app mod folder of its own, ready to use, in a card
folder at the top of this repo: `wasm-mods/` (the core mod), `wasm-todo/`
and `wasm-diff/`. Each card holds a `RECIPE.md`, a `preview.png` and
`mods/<name>/`. That mod folder holds the module (`<name>.wasm`), the same
bridge script (`wasm-bridge.js`), a `README.md` and a `host.lock.json`. The
lock pins the host it needs: its version and, for each machine, the download
URL and its SHA-256. A mod folder works wherever you copy it.

`wasm-git-graph` has no card and isn't in the gallery: it comes in the release's
tarball (its folder here is `modules/git-graph/mods/wasm-git-graph/`).

1. Add a card from Herdr's gallery, or copy `<card>/mods/<name>/` into
   herdr-app's `mods/` (`~/.config/herdr-app/mods/`, or
   `$HERDR_APP_HOME/mods/`). The release's `wasm-mods-<version>.tar.gz` holds the
   same mod folders, and `wasm-git-graph`. Then check each one:
   `herdr-app check ~/.config/herdr-app/mods/wasm-todo`.
2. Show one: `herdr-app do tab.new '{"mod": "wasm-todo"}'`, or put it in a
   layout block: `{"content": {"mod": "wasm-diff"}}`.
3. The first time, with no host on this machine, the pane says **WASM Mods
   core needed**. It names the host version it would install, from where, and
   the SHA-256 it must match. Nothing happens until you click:
   - **Download** fetches that file over HTTPS (`curl`) and installs it only
     if its SHA-256 is the pinned one;
   - **Build from source** (shown when `cargo` and the `wasm32-unknown-unknown`
     target are installed) runs `cargo install --locked --git <this repo>
     --tag <the release> wasm-mods-host` in a hidden terminal. It takes a few
     minutes.

The host goes to `~/.local/share/wasm-mods/host/<version>/`, beside any other
version, and every WASM mod on this machine shares it:

- Each mod uses the newest installed host that runs its module's protocol. It
  installs its own pinned host only when none does, and never replaces or
  removes one.
- A newer host installed later takes the new connections, and the older one
  exits once its panes close.
- A mod copies its `.wasm` into `~/.local/share/wasm-mods/modules/` when that
  copy differs.

Linux and macOS. Each release pins hosts for Linux x86_64 and aarch64, and
for macOS (Apple silicon and Intel) when they were built for it (see
RELEASING.md), from
`https://github.com/Calonca/herdr-wasm-mods/releases/download/v<version>/`.
Where a release has no host for the machine, **Build from source** builds it
with cargo. The Linux aarch64 host is built and checked but untested on real
aarch64 hardware.
The host runs on the machine the window runs on. A tab on another machine
needs nothing installed there: its git runs through `hc exec --on`.

### From source

Linux or macOS, with Rust 1.97 or newer and its `wasm32-unknown-unknown` target:

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --locked -p wasm-mods-host
(cd modules && cargo build --release --locked --target wasm32-unknown-unknown)
WITH_MODS=1 scripts/install.sh
```

`scripts/install.sh` installs:

- the host as `~/.local/share/wasm-mods/host/<version>/wasm-mods-host`, linked
  from `~/.local/bin/wasm-mods-host` for the command line;
- the modules into `~/.local/share/wasm-mods/modules/`;
- the core mod into herdr-app's `mods/`, and with `WITH_MODS=1` every WASM mod
  folder too, each with a lock for this build.

Running it again updates in place. Open panes reconnect to the new host, and a
changed module reloads where it's shown. `RELEASING.md` describes how a release
is made.

## What runs, and what it reads and writes

- **The mods** (JavaScript, `wasm-bridge.js` in each folder):
  - start the host in a hidden herdr-core terminal (`hc run --bg`). They talk
    to it over `127.0.0.1` on a port the system picks, with a random token, one
    connection per pane. The token goes through a file in a folder only you can
    open (`~/.cache/wasm-mods/run/`), never on a command line;
  - run small programs for that: `sha256sum` (`shasum` on macOS), `uname`,
    `cp`, `mv`, `mkdir`, `stat`, and `curl` or `cargo` only when you click
    Download or Build from source;
  - open a link a module offers with `xdg-open` (`open` on macOS) when you
    click it.
- **`wasm-mods-host`** runs the modules with wasmtime (no WASI: a module has no
  imports at all) and does what they're allowed to:
  - reads and writes files only inside folders you allowed, as above;
  - runs `git` only for a module allowed `git`, in the tab's folder, read-only
    subcommands only (`log`, `show`, `diff`, …). No shell. Options that would
    write a file, run another program or read outside the repo are refused, and
    external diff drivers, text conversion, fsmonitor and the pager are turned
    off whatever your git config says. Each run has a time and output limit;
  - runs `glab api projects/…` only for a module allowed `glab`, in the tab's
    folder: a GET, no other subcommand, method, field or option, and no path
    outside `projects/`. Same limits as git;
  - for a tab on another machine (a Herdr space's machine), runs that git or
    glab there with `hc exec --on <machine>`, under the same rules.
- **What they keep:** the hosts in `~/.local/share/wasm-mods/host/`, the
  modules in `~/.local/share/wasm-mods/modules/`, your answers in
  `~/.config/wasm-mods/permissions.toml`, compiled modules in
  `~/.cache/wasm-mods/wasmtime/`, and canvas pictures under herdr-app's
  `data/wasm-mods/`.
- **Network:** only the one download you click, from the URL in the mod's
  `host.lock.json` (HTTPS, redirects HTTPS only), or what `cargo` fetches for a
  build from source. Otherwise nothing connects anywhere but `127.0.0.1`.
- The host exits by itself once no pane has used it for ten minutes.

## Modules

- **todo** (`wasm-todo`): a list of things to do, kept in
  `~/.local/share/wasm-mods/todo/todo.json`. Asks to read and write that folder.
- **diff** (`wasm-diff`): what changed in the tab's repo: uncommitted,
  unpushed, the branch, or one commit (**commit ▾** lists the branch's recent
  commits). A file tree, and each file's diff, unified or side by side,
  syntax-coloured, with the changed words marked. PageUp/PageDown page through
  it; a shows all files one after another. It reads every file's changes at
  once and colours them in the background, so scrolling finds them ready. Its
  view toggles (all files, side by side, wrap) are kept for the next diff in
  `~/.local/share/wasm-mods/diff/settings.json`. Asks for `window.folder`,
  `git`, and to read and change files in that folder.

- **git graph** (`wasm-git-graph`, in the release's tarball, not in the gallery):
  the tab's repo as a commit graph, every branch and tag. Enter shows a commit's
  message and files, Space its diff (with `wasm-diff`). Whether the details show
  is kept in `~/.local/share/wasm-mods/git-graph/settings.json`. Asks for
  `window.folder`, `git`, and to read and change files in that folder.

`modules/` also holds the source of a module that isn't released as a mod:
`hello` (a counter and a canvas, no capabilities: the template for new
modules).

## Write your own module

A module is a Rust crate built for `wasm32-unknown-unknown` with the SDK
(`crates/wasm-mods-sdk`). Copy `modules/hello` (the template) and add it to
`modules/Cargo.toml`'s members:

```rust
use wasm_mods_sdk::*;

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Hello { clicks: u32 }

impl Frame for Hello {
    fn update(&mut self, input: &Input) -> Output {
        for event in &input.events {
            if matches!(event, Event::Clicked { id } if id == "more") {
                self.clicks += 1;
            }
        }
        Output::new("hello", column([
            heading("hello from WebAssembly"),
            muted(format!("{} clicks", self.clicks)),
            button("more", "click me"),
        ]))
    }
}

export_frame!(Hello);
manifest!(description: "a counter", capabilities: []);
```

- The host calls `update` with the pane's size, the theme and what happened
  since the last call. It gets back a tree of prebuilt components (columns, rows,
  text, text fields, lists, canvases, popups), which the bridge draws with
  Herdr's own elements, in Herdr's theme.
- The state is serialized, so a module keeps it when you install a new build
  while it's shown.
- Files and git go through effects the host answers in a later call (see the
  SDK docs: `cargo doc -p wasm-mods-sdk --open`), and only with the
  capabilities in your manifest.
- Unit tests run natively: `cd modules && cargo test`.

Give it a card and a mod folder: copy `wasm-todo/` to `wasm-<name>/`, rename
its `mods/wasm-todo` to `mods/wasm-<name>`, drop its `todo.wasm`, and in that
folder's `main.js` name the folder and the module
(`wasmMod({ folder: "wasm-<name>", module: "<name>" })`). The bridge is the
same file everywhere: edit the core mod's
(`wasm-mods/mods/wasm-mods/wasm-bridge.js`) and run `scripts/sync-bridge.sh`
to copy it into the others. Then build and install:

```sh
(cd modules && cargo build --release --target wasm32-unknown-unknown)
WITH_MODS=1 scripts/install.sh <name>
```

The bridge's own tests:
`node --import ./tests/js/register.mjs --test tests/js/bridge.test.mjs`.

## Layout

- `crates/wasm-mods-protocol`: what the host and a module exchange.
- `crates/wasm-mods-sdk`: the Rust SDK for modules.
- `crates/wasm-mods-host`: the host.
- `modules/`: the modules (their own workspace, built for wasm32), and
  `git-graph/mods/wasm-git-graph`, the mod folder that's in the release but has
  no card.
- `wasm-mods/`, `wasm-todo/`, `wasm-diff/`: the cards, one per mod: a
  recipe, a preview, and the mod folder (`mods/<name>/`) with the release's
  module and lock.
- `wasm-mods/mods/wasm-mods`: the core mod (hosts, permissions, any module by
  name). Its `wasm-bridge.js` is the bridge, copied into every mod folder.
- `scripts/`: `install.sh` (from source), `package.sh` (a release's mod
  folders, tarball and third-party licences), `sync-bridge.sh`,
  `licenses/` (what `THIRD-PARTY-LICENSES` is made from).
- `THIRD-PARTY-LICENSES`: the licences of the crates built into the host and
  into the modules, and of the syntax definitions in the diff module,
  attached to each release too.

## Known issues

- With many WASM panes open at once (about 16 or more on a 16-thread CPU),
  herdr-app's socket reads can stall the app for seconds. The fix is in
  herdr-app itself and isn't released yet.
- The aarch64 host is built and checked but untested on real aarch64
  hardware.

## License

MIT OR Apache-2.0, at your option. The host and the modules include
third-party crates under their own licences (MIT, Apache-2.0, Unicode-3.0,
Zlib), and the diff module includes syntax definitions from Sublime
Text's default packages under their own permissive licences (the
packages' licence, and MIT for a few): see `THIRD-PARTY-LICENSES`.
