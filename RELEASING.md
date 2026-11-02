# Releasing

A release is a tag `vX.Y.Z` with these files attached:

- `wasm-mods-host-x86_64` and `wasm-mods-host-aarch64`: the host, Linux, static (musl).
- `wasm-mods-host-macos-aarch64` and `wasm-mods-host-macos-x86_64`: the host, macOS,
  built on a Mac (step 2).
- `SHA256SUMS`: their checksums.
- `THIRD-PARTY-LICENSES`: the licences of the third-party code built into the host and
  the modules, and of the syntax definitions in the diff module (MIT and Apache ask for
  their notices to ship with the binaries).
- `wasm-mods-X.Y.Z.tar.gz`: the mod folders (`<card>/mods/wasm-*/`), each with its `.wasm`
  and a `host.lock.json` that pins this release's host by URL and sha256.

The cards (`wasm-mods/`, `wasm-todo/`, `wasm-diff/`) are committed with those `.wasm` files
and locks in them: Herdr's Mods gallery shows one card per top-level folder, with its
`RECIPE.md` and `preview.png`, and installs its `mods/<name>/` as it is.

A mod folder installs the host only when the person clicks Download (or Build from source)
in its "WASM Mods core needed" panel; the download is checked against the pinned sha256
before it's installed. So the URLs in the locks must be the release's, and the files
uploaded must be the ones the locks were made from.

## 1. Version

- `Cargo.toml`: `version = "X.Y.Z"` (the host reports it with `--version --json`).
- A new protocol: raise `PROTOCOL_VERSION` in `crates/wasm-mods-protocol/src/lib.rs`, and
  `PROTOCOL_MIN` only when the host stops running modules of the old one.
- Commit. The tag comes after packaging (4), which changes the mod folders.

## 2. Build the host, both architectures

With [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) (`cargo install
cargo-zigbuild`, plus `zig`):

```sh
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
for t in x86_64 aarch64; do
  cargo zigbuild --release --locked -p wasm-mods-host --target $t-unknown-linux-musl
done
mkdir -p dist/host
cp target/x86_64-unknown-linux-musl/release/wasm-mods-host  dist/host/wasm-mods-host-x86_64
cp target/aarch64-unknown-linux-musl/release/wasm-mods-host dist/host/wasm-mods-host-aarch64
dist/host/wasm-mods-host-x86_64 --version --json   # {"protocol":[min,max],"version":"X.Y.Z"}
```

The macOS hosts are built on a Mac, by the workflow `.github/workflows/macos-hosts.yml`:
push the commit to release, run the workflow on it (Actions → macOS hosts → Run
workflow, or `gh workflow run macos-hosts.yml --ref <branch>`), check its log (tests,
signature, `--version`), and put the files of its artifact in `dist/host`:

```sh
gh run download <run id> -n wasm-mods-host-macos -D dist/host
```

`package.sh` pins every host file it finds in `dist/host` and skips a missing one, so
a release without them pins Linux only. On a pushed tag the workflow builds them again
and adds them to that tag's release if they aren't there; those aren't the pinned
ones, so for a release whose locks pin macOS, upload the files from `dist/host`.

## 3. Build the modules

```sh
(cd modules && cargo build --release --locked --target wasm32-unknown-unknown)
```

## 4. Package

```sh
V=X.Y.Z
RELEASE_BASE=https://github.com/Calonca/herdr-wasm-mods/releases/download/v$V \
SOURCE_GIT=https://github.com/Calonca/herdr-wasm-mods SOURCE_TAG=v$V \
  scripts/package.sh dist/host
```

`package.sh` checks that every mod folder's `wasm-bridge.js` is the core mod's
(`wasm-mods/mods/wasm-mods/wasm-bridge.js`; fix with `scripts/sync-bridge.sh`), puts each
module's `.wasm` and a new `host.lock.json` in its mod folder, and writes
`dist/host/wasm-mods-$V.tar.gz`, `dist/host/SHA256SUMS` and `THIRD-PARTY-LICENSES` (in the
repo and in `dist/host/`, with [cargo-about](https://github.com/EmbarkStudios/cargo-about):
`cargo install --locked cargo-about`; its settings are in `scripts/licenses/`). Commit the changed mod
folders and `THIRD-PARTY-LICENSES`, then tag: `git tag vX.Y.Z`. `BUNDLE=1` also puts both host builds in each packed folder's
`bin/`, for an install without a download (it's installed only if its sha256 is the
pinned one). `package.sh` takes the host builds from the folder given, so the hashes in
the locks are those of the files you upload: keep that folder until the release is out.

To try the download before uploading, point the locks at the local files:
`RELEASE_BASE=file://$PWD/dist/host`, or keep the https locks and write a base URL to
`~/.config/wasm-mods/release-url` (the file name is kept from the lock's URL).

## 5. Upload

```sh
git push origin vX.Y.Z
gh release create vX.Y.Z --title vX.Y.Z --notes-file NOTES.md \
  dist/host/wasm-mods-host-x86_64 dist/host/wasm-mods-host-aarch64 \
  dist/host/wasm-mods-host-macos-aarch64 dist/host/wasm-mods-host-macos-x86_64 \
  dist/host/SHA256SUMS dist/host/THIRD-PARTY-LICENSES dist/host/wasm-mods-$V.tar.gz
```

(Leave out the macOS files when they weren't built.) Check one download: `curl -fsSL
<url> | sha256sum` matches `SHA256SUMS`.

## What people get

- Hosts live in `~/.local/share/wasm-mods/host/<version>/`, one folder per version, never
  overwritten or removed by a mod. Each mod uses the newest installed host whose protocol
  range runs it; a newer one that's installed later takes new connections, and the older
  one exits once its last one closes.
- Modules are copied into `~/.local/share/wasm-mods/modules/` when their sha256 differs.
- Removing a mod folder removes nothing else; the core screen (`wasm-mods`) removes a
  module and its answers.
