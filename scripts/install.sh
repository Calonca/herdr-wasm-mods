#!/bin/sh
# Installs what's built, for development and for this machine's own setup:
# - the host as ~/.local/share/wasm-mods/host/<version>/wasm-mods-host (moved into place
#   whole: a running host keeps its file), linked from ~/.local/bin/wasm-mods-host for the
#   command line;
# - the WASM mods to ${XDG_DATA_HOME:-~/.local/share}/wasm-mods/modules;
# - the core mod (wasm-mods: the permissions screen, and any module by name) to herdr-app's
#   mods folder, with a host.lock.json for this build. WITH_MODS=1 also installs every
#   other WASM mod folder (<card>/mods/wasm-*/: wasm-todo, …), each with its module and
#   a lock for this build.
#   scripts/install.sh            everything built
#   scripts/install.sh todo       one module (and the host and the core mod)
# Build first: cargo build --release -p wasm-mods-host, and in modules/:
# cargo build --release --target wasm32-unknown-unknown.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
bin="$HOME/.local/bin"
share="${XDG_DATA_HOME:-$HOME/.local/share}/wasm-mods"
modules="$share/modules"
app="${HERDR_APP_HOME:-$HOME/.config/herdr-app}"
built="$root/modules/target/wasm32-unknown-unknown/release"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
pmax=$(sed -n 's/^pub const PROTOCOL_VERSION: u32 = \([0-9]*\);/\1/p' "$root/crates/wasm-mods-protocol/src/lib.rs")
pmin=$(sed -n 's/^pub const PROTOCOL_MIN: u32 = \([0-9]*\);/\1/p' "$root/crates/wasm-mods-protocol/src/lib.rs")
hostdir="$share/host/$version"
# Linux or macOS: their own forms of a few commands.
case $(uname -s) in Darwin) os=macos ;; *) os=linux ;; esac
sha256() { if [ $os = macos ]; then shasum -a 256 "$1"; else sha256sum "$1"; fi | cut -d' ' -f1; }
mkdir -p "$bin" "$modules" "$hostdir" "$app/mods"
"$root/scripts/sync-bridge.sh" --check
install -m 755 "$root/target/release/wasm-mods-host" "$hostdir/.wasm-mods-host.new"
mv -f "$hostdir/.wasm-mods-host.new" "$hostdir/wasm-mods-host"
# The command line's (and older setups') path. A file there from before is replaced.
ln -sfn "$hostdir/wasm-mods-host" "$bin/.wasm-mods-host.new" && if [ $os = macos ]; then mv -f "$bin/.wasm-mods-host.new" "$bin/wasm-mods-host"; else mv -Tf "$bin/.wasm-mods-host.new" "$bin/wasm-mods-host"; fi
if [ $# -eq 0 ]; then set -- $(cd "$built" && ls *.wasm | sed 's/\.wasm$//'); fi
for name in "$@"; do
  file=$(echo "$name" | tr - _)
  # The host reloads a module when its file changes: moved into place whole.
  cp "$built/$file.wasm" "$modules/.$file.wasm.new"
  mv -f "$modules/.$file.wasm.new" "$modules/$file.wasm"
  echo "module $file"
done
arch=$(uname -m); [ "$arch" = arm64 ] && arch=aarch64
sum=$(sha256 "$hostdir/wasm-mods-host"); size=$(wc -c < "$hostdir/wasm-mods-host" | tr -d ' ')
lock() {
  printf '{\n  "host": {"version": "%s", "protocol": [%s, %s], "files": {"%s-%s": {"url": "file://%s", "sha256": "%s", "size": %s}}, "source": null},\n  "module": %s\n}\n' \
    "$version" "$pmin" "$pmax" "$os" "$arch" "$hostdir/wasm-mods-host" "$sum" "$size" "$1"
}
for dir in "$root"/*/mods/wasm-*/; do
  dir=${dir%/}; name=$(basename "$dir")
  [ -f "$dir/main.js" ] || continue
  [ "$name" = wasm-mods ] || [ "${WITH_MODS:-}" = 1 ] || continue
  mkdir -p "$app/mods/$name"
  cp -r "$dir/." "$app/mods/$name/"
  module=null
  if [ "$name" != wasm-mods ]; then
    m=$(echo "${name#wasm-}" | tr - _)
    cp "$built/$m.wasm" "$app/mods/$name/$m.wasm"
    module="{\"name\": \"$m\", \"protocol\": $pmax, \"sha256\": \"$(sha256 "$built/$m.wasm")\"}"
  fi
  lock "$module" > "$app/mods/$name/host.lock.json"
  echo "mod $name"
done
# Compiled now, so each loads at once when first shown.
"$hostdir/wasm-mods-host" --precompile
# Running hosts (older builds) go; the panes reconnect to the new one.
pkill -f "^$bin/wasm-mods-host --token-file" || true
pkill -f "^$share/host/[^ ]*/wasm-mods-host --token-file" || true
echo "host $hostdir/wasm-mods-host; mods in $app/mods"
