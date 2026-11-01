#!/bin/sh
# Readies the mod folders for a release, in place: each card's mod folder
# (<card>/mods/wasm-*/) gets its module's .wasm and a host.lock.json pinning this
# release's host. Then packs those folders into <hosts>/wasm-mods-X.Y.Z.tar.gz, writes
# <hosts>/SHA256SUMS, and writes THIRD-PARTY-LICENSES (the licences of the crates built
# into the host, with cargo-about) to the repo and to <hosts>.
#
#   RELEASE_BASE=https://github.com/Calonca/herdr-wasm-mods/releases/download/v0.1.0 \
#   SOURCE_GIT=https://github.com/Calonca/herdr-wasm-mods SOURCE_TAG=v0.1.0 \
#   scripts/package.sh dist/host
#
# dist/host holds the host builds, wasm-mods-host-x86_64 and wasm-mods-host-aarch64 (see
# RELEASING.md); their sha256s and sizes go in every lock, with RELEASE_BASE/<file> as the
# URL. SOURCE_GIT and SOURCE_TAG (or SOURCE_REV) are what "Build from source" builds.
# BUNDLE=1 also puts the host builds in each packed folder's bin/ (an offline install).
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
hosts=${1:?usage: scripts/package.sh <folder with wasm-mods-host-x86_64 and -aarch64>}
base=${RELEASE_BASE:?set RELEASE_BASE (https://…/download/vX.Y.Z)}
case "$base" in https://*|file:///*) ;; *) echo "RELEASE_BASE must be https://" >&2; exit 1 ;; esac
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
pmax=$(sed -n 's/^pub const PROTOCOL_VERSION: u32 = \([0-9]*\);/\1/p' "$root/crates/wasm-mods-protocol/src/lib.rs")
pmin=$(sed -n 's/^pub const PROTOCOL_MIN: u32 = \([0-9]*\);/\1/p' "$root/crates/wasm-mods-protocol/src/lib.rs")
built="$root/modules/target/wasm32-unknown-unknown/release"
"$root/scripts/sync-bridge.sh" --check
files=""
for arch in x86_64 aarch64; do
  f="$hosts/wasm-mods-host-$arch"
  if [ ! -f "$f" ]; then echo "skipping $arch: no $f" >&2; continue; fi
  sum=$(sha256sum "$f" | cut -d' ' -f1); size=$(stat -c %s "$f")
  files="$files${files:+,}\"linux-$arch\": {\"url\": \"${base%/}/wasm-mods-host-$arch\", \"sha256\": \"$sum\", \"size\": $size}"
done
[ -n "$files" ] || { echo "no host builds in $hosts" >&2; exit 1; }
source=null
if [ -n "${SOURCE_GIT:-}" ]; then
  if [ -n "${SOURCE_TAG:-}" ]; then ref="\"tag\": \"$SOURCE_TAG\""; else ref="\"rev\": \"${SOURCE_REV:?set SOURCE_TAG or SOURCE_REV}\""; fi
  source="{\"git\": \"$SOURCE_GIT\", $ref}"
fi
host="{\"version\": \"$version\", \"protocol\": [$pmin, $pmax], \"files\": {$files}, \"source\": $source}"
pack=$(mktemp -d); trap 'rm -rf "$pack"' EXIT
for dir in "$root"/*/mods/wasm-*/; do
  dir=${dir%/}; name=$(basename "$dir")
  [ -f "$dir/main.js" ] || continue
  module=null
  if [ "$name" != wasm-mods ]; then
    m=$(echo "${name#wasm-}" | tr - _)
    cp "$built/$m.wasm" "$dir/$m.wasm"
    module="{\"name\": \"$m\", \"protocol\": $pmax, \"sha256\": \"$(sha256sum "$built/$m.wasm" | cut -d' ' -f1)\"}"
  fi
  printf '{\n  "host": %s,\n  "module": %s\n}\n' "$host" "$module" > "$dir/host.lock.json"
  cp -r "$dir" "$pack/$name"
  if [ "${BUNDLE:-}" = 1 ]; then
    mkdir -p "$pack/$name/bin"
    for arch in x86_64 aarch64; do [ -f "$hosts/wasm-mods-host-$arch" ] && cp "$hosts/wasm-mods-host-$arch" "$pack/$name/bin/"; done
  fi
  echo "$name"
done
# The licences of every crate built into the host, for both targets (scripts/licenses/).
cargo about generate --frozen --fail -m "$root/crates/wasm-mods-host/Cargo.toml" \
  -c "$root/scripts/licenses/about.toml" -o "$root/THIRD-PARTY-LICENSES" "$root/scripts/licenses/about.hbs"
cp "$root/THIRD-PARTY-LICENSES" "$hosts/THIRD-PARTY-LICENSES"
tar -C "$pack" --sort=name --owner=0 --group=0 --numeric-owner --mtime=@0 -czf "$hosts/wasm-mods-$version.tar.gz" .
(cd "$hosts" && sha256sum wasm-mods-host-* > SHA256SUMS)
echo "mod folders ready; $hosts/wasm-mods-$version.tar.gz; checksums in $hosts/SHA256SUMS; $hosts/THIRD-PARTY-LICENSES"
