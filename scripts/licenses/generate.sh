#!/bin/sh
# Writes THIRD-PARTY-LICENSES (to the path given, default the repo's): the licences of
# every third-party crate built into the host (both musl targets) and into the modules
# (wasm32), with cargo-about, and of the syntax definitions the diff module embeds
# (the packages' licence, sublime-packages.txt, and the files with their own,
# sublime-syntaxes.txt).
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
here="$root/scripts/licenses"
out=${1:-$root/THIRD-PARTY-LICENSES}
about() { cargo about generate --frozen --fail -c "$here/about.toml" "$@" "$here/about.hbs"; }
{
  cat <<'HEAD'
Third-party licences

WASM Mods is MIT OR Apache-2.0. What a release ships is built from the third-party
code below, under these licences: the host (wasm-mods-host), the modules (the .wasm
files in the mod folders), and the syntax definitions the diff module embeds.
HEAD
  printf '\n================================================================================\n'
  printf 'wasm-mods-host (x86_64 and aarch64, musl)\n'
  printf '================================================================================\n\n'
  about -m "$root/crates/wasm-mods-host/Cargo.toml" --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl
  printf '\n================================================================================\n'
  printf 'The modules (wasm32)\n'
  printf '================================================================================\n\n'
  about -m "$root/modules/Cargo.toml" --workspace --target wasm32-unknown-unknown
  printf '\n================================================================================\n'
  printf "Syntax definitions in the diff module\n"
  printf '================================================================================\n\n'
  printf '%s\n%s\n\n' "The diff module colours code with syntect's default syntax definitions, which come" \
    "from Sublime Text's default packages. Their licence:"
  cat "$here/sublime-packages.txt"
  printf '\n%s\n%s\n\n' "Some of those syntax definitions carry their own licence, the MIT License (checked" \
    "at the packages' commit syntect 5.3.0 is built from):"
  cat "$here/sublime-syntaxes.txt"
} > "$out.tmp"
mv "$out.tmp" "$out"
