// wasm-hello: the hello WASM mod. All the work is in ./wasm-bridge.js, the same file in
// every WASM mod folder, with the module (./hello.wasm) and the host it needs
// (./host.lock.json) beside it.
import { wasmMod } from "./wasm-bridge.js";

export default wasmMod({ folder: "wasm-hello", module: "hello" });
