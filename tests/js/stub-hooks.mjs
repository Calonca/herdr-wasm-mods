import { readFileSync } from "node:fs";

const STUBS = ["herdr", "gpui-kit", "gpui-base"];
// The names the mods' own files take from the app's modules.
const core = "../../wasm-mods/mods/wasm-mods";
const bridge = [`${core}/wasm-bridge.js`, `${core}/core.js`, `${core}/main.js`]
  .map((f) => readFileSync(new URL(f, import.meta.url), "utf8")).join("\n");

function names(spec) {
  const out = new Set();
  for (const m of bridge.matchAll(/import\s*\{([^}]*)\}\s*from\s*"([^"]+)"/g)) {
    if (m[2] === spec) m[1].split(",").map((s) => s.trim()).filter(Boolean).forEach((n) => out.add(n));
  }
  return [...out];
}

export async function resolve(spec, ctx, next) {
  if (STUBS.includes(spec)) return { url: `stub:${spec}`, shortCircuit: true };
  return next(spec, ctx);
}

export async function load(url, ctx, next) {
  if (!url.startsWith("stub:")) return next(url, ctx);
  const src = names(url.slice(5)).map((n) => n === "View"
    ? "export class View {}"
    : `export const ${n} = new Proxy(function () {}, { get: () => () => {}, apply: () => undefined });`).join("\n");
  return { format: "module", source: src, shortCircuit: true };
}
