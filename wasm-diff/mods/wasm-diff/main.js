// wasm-diff: the diff WASM mod. All the work is in ./wasm-bridge.js, the same file in
// every WASM mod folder, with the module (./diff.wasm) and the host it needs
// (./host.lock.json) beside it.
import { wasmMod } from "./wasm-bridge.js";

export default wasmMod({ folder: "wasm-diff", module: "diff" });
