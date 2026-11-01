// wasm-mods: the WASM Mods core mod.
//
// Opened on its own (`mod.open {name: "wasm-mods"}`), it's the core screen: the hosts
// installed, installing one, and every module's permissions (./core.js). Given a module
// (`tab.new {mod: "wasm-mods", props: {module: "todo"}}`, or a layout block), it runs that
// installed module like the module's own mod folder would (./wasm-bridge.js).

import { View } from "gpui-kit";
import { props } from "herdr";
import { wasmMod } from "./wasm-bridge.js";
import { CoreScreen } from "./core.js";

const Module = wasmMod({ folder: "wasm-mods", core: true });

export default class WasmModsCore extends View {
  init(p, cx) {
    // `new` runs the view's init already (herdr's View constructor): it isn't called
    // again here, or a module would connect twice.
    this.inner = props()?.module ? new Module(p) : new CoreScreen(p);
    if (!this.inner.inited) this.inner.init(p, cx);
  }

  render(cx) {
    return this.inner.render(cx);
  }

  close(cx) {
    return this.inner.close?.(cx);
  }
}
