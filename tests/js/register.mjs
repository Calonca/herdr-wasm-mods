// Lets node import the bridge (wasm-mods/mods/wasm-mods/wasm-bridge.js): Herdr's own
// modules ("herdr", "gpui-kit", "gpui-base") become stubs exporting the names it imports.
import { register } from "node:module";
register("./stub-hooks.mjs", import.meta.url);
