#!/bin/sh
# Copies the core mod's wasm-bridge.js (wasm-mods/mods/wasm-mods/wasm-bridge.js, the one to
# edit) into every other WASM mod folder (<card>/mods/wasm-*/,
# modules/<module>/mods/wasm-*/). `--check`: change
# nothing, exit 1 if a copy differs.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
src="$root/wasm-mods/mods/wasm-mods/wasm-bridge.js"
check=false; [ "${1:-}" = --check ] && check=true
bad=0
for dir in "$root"/*/mods/wasm-*/ "$root"/modules/*/mods/wasm-*/; do
  dir=${dir%/}
  [ -f "$dir/main.js" ] || continue
  [ "$dir/wasm-bridge.js" = "$src" ] && continue
  if $check; then
    cmp -s "$src" "$dir/wasm-bridge.js" || { echo "out of sync: $dir/wasm-bridge.js"; bad=1; }
  else
    cp "$src" "$dir/wasm-bridge.js"
  fi
done
exit $bad
