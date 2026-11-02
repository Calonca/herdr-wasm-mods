// wasm-bridge.js: shows a WASM mod (a sandboxed WebAssembly module) in a pane or a block.
// Every WASM mod folder has an identical copy of this file; its main.js names the
// module:
//
//   export default wasmMod({ folder: "wasm-todo", module: "todo" });
//
//   tab.new {mod: "wasm-todo"}
//   a layout block: {"content": {"mod": "wasm-todo"}}
//
// The core mod (`wasm-mods`) runs any installed module named in its props:
//   tab.new {mod: "wasm-mods", props: {module: "todo"}}
//
// The modules run in wasm-mods-host, a small program this mod starts in a hidden
// herdr-core terminal and talks to over a socket on 127.0.0.1 (one connection per copy,
// opened with a token only this mod knows). The host enforces each module's
// permissions; this mod draws what the module returns (a tree of components) with
// GPUI elements and sends back what the user does.
//
// Props: `module` (the core mod: a name in ~/.local/share/wasm-mods/modules), `arg` (what it's opened
// for), `open` (a file it's opened with, which it may then read, or a link).

import { View, div, image, PathBuilder } from "gpui-kit";
import { v_flex, h_flex, InputState, Input, VirtualListScrollHandle, v_virtual_list, Scrollbar } from "gpui-base";
import { exec, hc, home, appHome, props, theme, settings, run, readTextFile, writeTextFile, panes, focused, spaces } from "herdr";
import * as herdr from "herdr";
import net from "net";
import { randomBytes } from "crypto";
import { Buffer } from "buffer";

/** The app draws WASM mods itself (`herdr.wasmView()`, 1 or more: `WasmViewState` and
 *  `WasmView` in "herdr-components", with `lost()` and `connect(port, token)`): the pane
 *  is then handed to it. Checked without importing anything, so on an app without it
 *  nothing waits and this script draws, as it always does. */
const NATIVE_VIEW = typeof herdr.wasmView === "function" && Number(herdr.wasmView()) >= 1;

/** The app's zoom (settings `zoom`): it scales every length a script writes (a path's
 *  points and a list's item sizes too), but the sizes and positions it reports (size
 *  changes, pointer events, wheel deltas) come unscaled. What it reports is divided by
 *  this, so everything here (and in the protocol) is in the script's own pixels. */
function uiZoom() {
  const z = Number(settings()?.zoom);
  return z > 0 ? z : 1;
}
/** A reported point or size in the script's pixels. */
const unzoomed = (p) => { const z = uiZoom(); return p && { x: p.x / z, y: p.y / z }; };

/** Tab and Shift+Tab, as this mod's own actions while its view has the keyboard: the
 *  shell binds them to moving the keyboard on (a binding wins over a key handler, so
 *  the view's never heard them); a binding in the view's own context comes first. */
const KEY_CONTEXT = "WasmMods";
function bindTab(cx) {
  if (globalThis.__wasmModsTabBound) return;
  try {
    cx.bind_keys([
      { keystroke: "tab", action: "wasm-mods::tab", context: KEY_CONTEXT },
      { keystroke: "shift-tab", action: "wasm-mods::tab-back", context: KEY_CONTEXT },
    ]);
    globalThis.__wasmModsTabBound = true;
  } catch (e) {
    console.warn(`wasm-mods: no Tab binding: ${e?.message ?? e}`);
  }
}

// ---------------------------------------------------------------------------
// The core: hosts, modules and the lock
// ---------------------------------------------------------------------------
//
// Every WASM mod folder (mods/wasm-<module>/) carries this file, its module's .wasm and
// host.lock.json: the host version it was built and tested with, the protocols that host
// runs, and where to get it for each machine (an HTTPS URL and its sha256). Hosts are
// installed side by side, ~/.local/share/wasm-mods/host/<version>/wasm-mods-host, never
// overwritten or removed by a mod; every mod uses the newest installed host that runs its
// module's protocol, and installs its pinned one only when none does. Modules go to
// ~/.local/share/wasm-mods/modules, where every host finds them.

/** Shared by every mod and Herdr app home on this machine. */
const SHARE = () => `${home()}/.local/share/wasm-mods`;
export const HOSTS = () => `${SHARE()}/host`;
const MODULES = () => `${SHARE()}/modules`;
/** Overrides for testing a release before it's published (file: the base URL the lock's
 *  file names are fetched from instead). */
const RELEASE_URL_FILE = () => `${home()}/.config/wasm-mods/release-url`;
/// Where the host puts pictures for canvases: under the app's folder, where `image()` can
/// load them, and not in this mod's own folder (a change there reloads the mod).
const IMAGES = "data/wasm-mods/images";
const IMAGE_DIR = () => `${appHome()}/${IMAGES}`;
/** This Herdr app home's own run folder (one host per app home): only the user can open
 *  it, and the token goes there, never on a command line. */
export const RUN_DIR = () => `${home()}/.cache/wasm-mods/run/${appKey()}`;
/** A mod folder of this app home. */
const MOD_DIR = (folder) => `${appHome()}/mods/${folder}`;

/** A short name for this app home (FNV-1a, twice: not for security, just distinct). */
function appKey() {
  let a = 0x811c9dc5, b = 0x01000193;
  for (const ch of String(appHome())) {
    const c = ch.codePointAt(0);
    a = Math.imul(a ^ c, 0x01000193) >>> 0;
    b = Math.imul(b ^ c, 0x811c9dc5) >>> 0;
  }
  return a.toString(16).padStart(8, "0") + b.toString(16).padStart(8, "0");
}

/** `1.2.3` (a `-pre` part is ignored) as numbers, else null. */
export function parseVersion(v) {
  const m = /^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$/.exec(String(v ?? "").trim());
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
}

/** <0, 0, >0 as `a` is older, the same, newer than `b`; an unreadable version is oldest. */
/** "protocol 4", or "protocols 4–5". */
export function protocols([min, max]) {
  return min === max ? `protocol ${min}` : `protocols ${min}–${max}`;
}

export function compareVersions(a, b) {
  const x = parseVersion(a), y = parseVersion(b);
  if (!x || !y) return (x ? 1 : 0) - (y ? 1 : 0);
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] - y[i];
  return 0;
}

/** Whether `host` ({protocol: [min, max]}) runs modules built for `protocol`. */
export function fits(host, protocol) {
  const [min, max] = host?.protocol ?? [];
  return Number.isInteger(protocol) && protocol >= min && protocol <= max;
}

/** The newest of `hosts` ([{version, protocol, path}]) that runs `protocol`, else null. */
export function pickHost(hosts, protocol) {
  return hosts.filter((h) => parseVersion(h.version) && fits(h, protocol))
    .sort((a, b) => compareVersions(b.version, a.version))[0] ?? null;
}

const SHA256 = /^[0-9a-f]{64}$/;

/** The hash in `sha256sum`'s output (`<hex>  <file>`), else null. */
export function parseSha256sum(out) {
  const h = String(out ?? "").trim().split(/\s+/)[0]?.toLowerCase();
  return SHA256.test(h ?? "") ? h : null;
}

/** Whether two sha256 hashes are the same (both well formed). */
export function sameHash(a, b) {
  const x = String(a ?? "").toLowerCase(), y = String(b ?? "").toLowerCase();
  return SHA256.test(x) && x === y;
}

/** host.lock.json, checked: {host: {version, protocol: [min, max], files: {"linux-x86_64"
 *  (or linux-aarch64, macos-aarch64, macos-x86_64):
 *  {url, sha256, size}, …}, source: {git, tag | rev} | null}, module: {name, protocol,
 *  sha256}}. Throws with what's wrong. URLs must be https:// (file:// for local tests). */
export function parseLock(text) {
  let j;
  try { j = JSON.parse(text); } catch (e) { throw new Error(`host.lock.json isn't JSON: ${e.message}`); }
  const h = j?.host, m = j?.module;
  if (!parseVersion(h?.version)) throw new Error("host.lock.json: host.version isn't a version (1.2.3)");
  const p = h.protocol;
  if (!Array.isArray(p) || p.length !== 2 || !p.every(Number.isInteger) || p[0] > p[1]) {
    throw new Error("host.lock.json: host.protocol isn't [min, max]");
  }
  const files = {};
  for (const [target, f] of Object.entries(h.files ?? {})) {
    if (!/^(linux|macos)-(x86_64|aarch64)$/.test(target)) continue;
    if (!/^(https:\/\/|file:\/\/\/)[^\s]+$/.test(f?.url ?? "")) throw new Error(`host.lock.json: ${target}'s url must be https://`);
    if (!SHA256.test(String(f.sha256 ?? "").toLowerCase())) throw new Error(`host.lock.json: ${target}'s sha256 isn't 64 hex digits`);
    files[target] = { url: f.url, sha256: f.sha256.toLowerCase(), size: Number(f.size) || null };
  }
  let source = null;
  if (h.source?.git) {
    if (!/^(https:\/\/|file:\/\/\/)[^\s]+$/.test(h.source.git)) throw new Error("host.lock.json: host.source.git must be https://");
    if (!h.source.tag && !h.source.rev) throw new Error("host.lock.json: host.source needs a tag or a rev");
    source = { git: h.source.git, tag: h.source.tag ?? null, rev: h.source.rev ?? null };
  }
  if (m != null) {
    if (!/^[a-z0-9_-]+$/.test(m.name ?? "") || !Number.isInteger(m.protocol)) throw new Error("host.lock.json: module needs a name and a protocol");
    if (m.sha256 != null && !SHA256.test(String(m.sha256).toLowerCase())) throw new Error("host.lock.json: module.sha256 isn't 64 hex digits");
  }
  return {
    host: { version: h.version, protocol: [p[0], p[1]], files, source },
    module: m ? { name: m.name, protocol: m.protocol, sha256: m.sha256?.toLowerCase() ?? null } : null,
  };
}

/** The run file: "<port> <token> [<version>]" (no version: a host from before releases, 0.0.0). */
export function parseRunFile(text) {
  const m = /^(\d+) ([0-9a-f]{48})(?: (\S+))?\n$/.exec(text ?? "");
  return m ? { port: Number(m[1]), token: m[2], version: m[3] ?? "0.0.0" } : null;
}

/** Whether a copy that needs `protocol` should start `best` instead of using the host
 *  that runs (`running`: {version}, `hosts`: what's installed, to know its protocols). */
export function shouldTakeOver(running, best, hosts, protocol) {
  if (!best) return false;
  const info = hosts.find((h) => h.version === running.version) ?? { version: running.version, protocol: [4, 4] };
  return !fits(info, protocol) || compareVersions(best.version, running.version) > 0;
}

/** A machine as host.lock.json names it, from `uname -sm` ("Linux x86_64" →
 *  `linux-x86_64`, "Darwin arm64" → `macos-aarch64`), else null. */
export function parseTarget(unameSm) {
  const [os, arch] = String(unameSm ?? "").trim().split(/\s+/);
  const a = arch === "arm64" ? "aarch64" : arch;
  if (!["x86_64", "aarch64"].includes(a)) return null;
  return os === "Linux" ? `linux-${a}` : os === "Darwin" ? `macos-${a}` : null;
}

/** This machine, as host.lock.json names it (`linux-x86_64`), else null. Asked once. */
export function target() {
  return (globalThis.__wasmModsTarget ??= exec("uname", ["-sm"]).then((s) => parseTarget(s.stdout))
    .catch((e) => { delete globalThis.__wasmModsTarget; throw e; }));
}

/** On macOS: its own forms of the commands below (BSD stat and mv, shasum, open). */
const onMac = async () => String((await target().catch(() => null)) ?? "").startsWith("macos-");

async function sha256(path) {
  const r = (await onMac()) ? await exec("shasum", ["-a", "256", path]) : await exec("sha256sum", [path]);
  return r.code === 0 ? parseSha256sum(r.stdout) : null;
}

/** Opens a link or a file with the desktop's own handler. */
async function openWithDesktop(target) {
  return exec((await onMac()) ? "open" : "xdg-open", [target]);
}

/** The lock of mod folder `folder`, or the core's, parsed (null if neither has one). */
export async function readLock(folder) {
  // Read once per folder while the app runs: a new pane then runs no command before it
  // connects (see lookForNewer).
  const cache = (globalThis.__wasmModsLocks ??= {});
  if (!(folder in cache)) cache[folder] = readLockFile(folder).catch((e) => { delete cache[folder]; throw e; });
  return cache[folder];
}

async function readLockFile(folder) {
  for (const f of [folder, "wasm-mods"]) {
    if (!f) continue;
    // Not readTextFile: inside a mod folder it can stay unanswered (seen on 0.1.11).
    const r = await exec("cat", [`${MOD_DIR(f)}/host.lock.json`]);
    if (r.code === 0 && r.stdout) return parseLock(r.stdout);
  }
  return null;
}

/** Every installed host: [{version, protocol, path}], asked of each (`--version --json`).
 *  Kept while the host folder lists the same versions (one `ls` a call): a host
 *  installed or removed meanwhile, by a mod or by hand, is seen. */
export async function installedHosts() {
  const g = (globalThis.__wasmModsHosts ??= { key: null, list: null });
  const ls = await exec("sh", ["-c", 'cd "$0" 2>/dev/null && ls -1 */wasm-mods-host 2>/dev/null', HOSTS()]);
  const key = String(ls.stdout ?? "");
  if (g.list && g.key === key) return g.list;
  const out = [];
  for (const v of key.split("\n").map((l) => l.split("/")[0]).filter((v) => parseVersion(v))) {
    const path = `${HOSTS()}/${v}/wasm-mods-host`;
    const r = await exec(path, ["--version", "--json"]).catch(() => null);
    try {
      const j = JSON.parse(r?.stdout ?? "");
      if (j.version === v && Array.isArray(j.protocol)) out.push({ version: v, protocol: j.protocol, path });
    } catch { /* not a host, or a broken one: skipped */ }
  }
  Object.assign(g, { key, list: out });
  return out;
}

export function hostsChanged() {
  if (globalThis.__wasmModsHosts) globalThis.__wasmModsHosts.list = null;
}

/** The newest installed host: the one the permissions screen asks. */
export async function newestHost() {
  return (await installedHosts()).sort((a, b) => compareVersions(b.version, a.version))[0] ?? null;
}

/** No installed host runs this module: what to offer (see the panel). */
class CoreNeeded extends Error {
  constructor(need) {
    super("WASM Mods core needed");
    this.need = need;
  }
}

/** Copies mod folder `folder`'s `<module>.wasm` into the shared modules folder when what's
 *  there differs (written beside, then renamed: a host reloads it whole). */
/** The name a mod folder's module is installed under in the shared modules folder: the
 *  folder's own name (herdr-app keeps those apart, renaming a folder whose name is
 *  taken), so two authors' modules both called "diff" don't replace each other. */
export function installedName(folder) {
  const name = String(folder).toLowerCase().replace(/[^a-z0-9_-]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 64);
  return name || "wasm-mod";
}

/** Copies mod folder `folder`'s `<module>.wasm` into the shared modules folder (only if
 *  its sha256 differs): the name the host knows it by. Without a .wasm in the folder,
 *  the shared `<module>.wasm` is used as it is. */
async function installModule(folder, module) {
  const own = `${MOD_DIR(folder)}/${module}.wasm`;
  const mine = await sha256(own);
  if (!mine) return module;
  const name = installedName(folder);
  const shared = `${MODULES()}/${name}.wasm`;
  if (sameHash(mine, await sha256(shared))) return name;
  await exec("mkdir", ["-p", MODULES()]);
  const tmp = `${MODULES()}/.${name}.wasm.${randomBytes(4).toString("hex")}`;
  const cp = await exec("cp", [own, tmp]);
  if (cp.code !== 0) throw new Error(`can't copy ${module}.wasm: ${cp.stderr}`);
  const mv = await exec("mv", ["-f", tmp, shared]);
  if (mv.code !== 0) throw new Error(`can't install ${module}.wasm: ${mv.stderr}`);
  return name;
}

/** Puts `file` (already checked) in place as host `version`: in a folder of its own, made
 *  beside and renamed, so a host is never seen half written; a version already there is
 *  kept as it is. */
export async function placeHost(file, version) {
  await exec("mkdir", ["-p", HOSTS()]);
  const tmp = `${HOSTS()}/.${version}.${randomBytes(4).toString("hex")}`;
  await exec("mkdir", ["-p", tmp]);
  const steps = [["cp", [file, `${tmp}/wasm-mods-host`]], ["chmod", ["755", `${tmp}/wasm-mods-host`]]];
  for (const [cmd, args] of steps) {
    const r = await exec(cmd, args);
    if (r.code !== 0) { await exec("rm", ["-rf", tmp]); throw new Error(`can't install the host: ${r.stderr}`); }
  }
  // -T: fails if the version is there already (another copy won): then that one stays.
  // macOS's mv has no -T: it moves only if the version isn't there.
  if (await onMac()) await exec("sh", ["-c", '[ -e "$1" ] || mv "$0" "$1"', tmp, `${HOSTS()}/${version}`]);
  else await exec("mv", ["-T", tmp, `${HOSTS()}/${version}`]);
  await exec("rm", ["-rf", tmp]);
  hostsChanged();
}

/** The host the folder ships (bin/wasm-mods-host-<arch>), installed if its sha256 is the
 *  lock's. True if it was. */
async function installBundled(folder, lock, tgt) {
  const want = lock.host.files[tgt]?.sha256;
  // Named as the release names it: wasm-mods-host-x86_64 (Linux), wasm-mods-host-macos-aarch64.
  const file = `${MOD_DIR(folder)}/bin/wasm-mods-host-${tgt.replace("linux-", "")}`;
  if (!want || !sameHash(await sha256(file), want)) return false;
  await placeHost(file, lock.host.version);
  return true;
}

/** Where the host for `tgt` comes from: the lock's URL, or the same file name under the
 *  base URL in RELEASE_URL_FILE. */
async function hostUrl(lock, tgt) {
  const url = lock.host.files[tgt].url;
  const base = String((await readTextFile(RELEASE_URL_FILE()).catch(() => null)) ?? "").trim();
  if (!base) return url;
  if (!/^(https:\/\/|file:\/\/\/)\S+$/.test(base)) throw new Error(`${RELEASE_URL_FILE()}: not an https:// URL`);
  return `${base.replace(/\/+$/, "")}/${url.split("/").pop()}`;
}

/** Downloads the lock's host for `tgt` (HTTPS only), checks its sha256, installs it. */
export async function downloadHost(lock, tgt) {
  const url = await hostUrl(lock, tgt);
  const dir = `${HOSTS()}/.download-${randomBytes(4).toString("hex")}`;
  await exec("mkdir", ["-p", dir]);
  try {
    const file = `${dir}/wasm-mods-host`;
    const r = await exec("curl", ["--proto", "=https,file", "--proto-redir", "=https", "--tlsv1.2", "-fsSL",
      "--max-filesize", "104857600", "--connect-timeout", "20", "--max-time", "300", "-o", file, url]);
    if (r.code !== 0) throw new Error(`download failed: ${(r.stderr || `curl exited ${r.code}`).trim()}`);
    const got = await sha256(file);
    if (!sameHash(got, lock.host.files[tgt].sha256)) {
      throw new Error(`the download's sha256 (${got ?? "none"}) isn't the pinned one: not installed`);
    }
    await placeHost(file, lock.host.version);
  } finally {
    await exec("rm", ["-rf", dir]);
  }
}

/** Whether this machine can build the host: cargo, and the wasm32 target (for modules). */
async function canBuild() {
  // An app started from the macOS Dock doesn't get the shell's PATH.
  const r = await exec("sh", ["-c", 'PATH="$HOME/.cargo/bin:$PATH"; command -v cargo >/dev/null && rustup target list --installed 2>/dev/null | grep -qx wasm32-unknown-unknown']);
  return r.code === 0;
}

/** Builds the lock's host from its source (cargo install, in a hidden terminal: it takes
 *  minutes) and installs it. `progress(text)` is told how it goes. */
export async function buildHost(cx, lock, progress) {
  const src = lock.host.source;
  const dir = `${HOSTS()}/.build-${randomBytes(4).toString("hex")}`;
  await exec("mkdir", ["-p", dir]);
  const ref = src.tag ? ["--tag", src.tag] : ["--rev", src.rev];
  const cmd = ["cargo", "install", "--locked", "--git", src.git, ...ref, "--root", dir, "wasm-mods-host"];
  const quoted = cmd.map((a) => `'${a.replace(/'/g, "'\\''")}'`).join(" ");
  const r = await hc(["run", "--bg", "--json", "--no-parent", "--rm", "--", "sh", "-c",
    `PATH="$HOME/.cargo/bin:$PATH"; ${quoted} > '${dir}/build.log' 2>&1; echo $? > '${dir}/done'`]);
  if (r.code !== 0) throw new Error(`can't start the build: ${r.stderr || r.stdout}`);
  for (let i = 0; ; i++) {
    const done = await readTextFile(`${dir}/done`).catch(() => null);
    if (done != null && done.trim() !== "") {
      if (done.trim() !== "0") {
        const log = await exec("tail", ["-n", "5", `${dir}/build.log`]);
        throw new Error(`the build failed (log: ${dir}/build.log): ${log.stdout.trim()}`);
      }
      break;
    }
    if (i % 10 === 0) {
      const log = await exec("tail", ["-n", "1", `${dir}/build.log`]);
      progress(`building… ${String(log.stdout ?? "").trim().slice(0, 90)}`);
    }
    await delay(cx, 1000);
  }
  const built = `${dir}/bin/wasm-mods-host`;
  const v = await exec(built, ["--version", "--json"]);
  let j = null;
  try { j = JSON.parse(v.stdout); } catch { /* checked below */ }
  if (j?.version !== lock.host.version) throw new Error(`built ${j?.version ?? "something else"}, not ${lock.host.version}: not installed`);
  await placeHost(built, lock.host.version);
  await exec("rm", ["-rf", dir]);
}

/** Where the host listens, for this copy: every copy of the mod runs on its own, so they
 *  find the one host through HOST_FILE. */
function hostState() {
  const key = `__wasmModsHost_${appKey()}`;
  globalThis[key] ??= { port: 0, token: "", version: "", protocol: null };
  return globalThis[key];
}

/** "<port> <token> <version>" of the host every copy of this app home uses. The host
 *  writes it as soon as it listens (`--host-file`), so the copies find it even if the
 *  copy that started it was closed or reloaded meanwhile (its timers stop, and with them
 *  its start), and removes it when it exits idle. */
const HOST_FILE = () => `${RUN_DIR()}/host`;
/** Made (mkdir: one copy wins) by the copy starting the host. */
const START_LOCK = () => `${RUN_DIR()}/starting`;
/** A start not finished within this is taken over (its copy was closed or reloaded). */
const START_MAX = 15000;

function delay(cx, ms) {
  return new Promise((resolve) => cx.timer.after(ms, resolve));
}

/** Resolves with where a host that runs `protocol` listens: the one this app's copies
 *  use, else what the run file names, else one this copy starts (the newest installed
 *  that runs it). Throws CoreNeeded when none is installed.
 *
 *  Each copy looks for itself: the start lock and the run file (which the host writes)
 *  keep them to one host. No copy waits on another's start: a copy in a tab that isn't
 *  shown, or one left behind by a reload, may never get its timers again. */
async function ensureHost(cx, protocol, need) {
  const h = hostState();
  const { port, token } = h;
  if (port && token && h.protocol && fits(h, protocol)) {
    lookForNewer(h, protocol);
    return { port, token };
  }
  const got = await findHost(cx, protocol, need);
  Object.assign(h, got);
  return got;
}

/** A newer host installed since (one `ls`): the next connection goes to it (findHost),
 *  and the one running idles out. Looked for in the background: on herdr-app 0.1.11 a
 *  command's answer can wait until something else wakes the app, and a pane connecting
 *  (or reconnecting) to the host it knows mustn't wait with it. */
function lookForNewer(h, protocol) {
  if (h.looking) return;
  const { port, version } = h;
  h.looking = installedHosts()
    .then((hosts) => {
      const best = pickHost(hosts, protocol);
      if (best && h.port === port && compareVersions(best.version, version || "0.0.0") > 0) {
        Object.assign(h, { port: 0, token: "", version: "", protocol: null });
      }
    })
    .catch(() => {})
    .finally(() => { h.looking = null; });
}

async function findHost(cx, protocol, need) {
  const hosts = await installedHosts();
  const best = pickHost(hosts, protocol);
  if (!best) throw new CoreNeeded(need);
  const dir = RUN_DIR();
  const mk = await exec("mkdir", ["-p", "-m", "700", dir]);
  if (mk.code !== 0) throw new Error(`can't make ${dir}: ${mk.stderr}`);
  // -p doesn't change a folder that was already there.
  await exec("chmod", ["700", `${home()}/.cache/wasm-mods/run`, dir]);
  const file = HOST_FILE(), lock = START_LOCK();
  const usable = (run) => run && !shouldTakeOver(run, best, hosts, protocol);
  const found = (run) => ({ port: run.port, token: run.token, version: run.version,
    protocol: (hosts.find((x) => x.version === run.version) ?? best).protocol });
  const waitUntil = Date.now() + START_MAX + 5000;
  while (Date.now() < waitUntil) {
    const run = parseRunFile(await readTextFile(file).catch(() => null));
    if (usable(run)) return found(run);
    if ((await exec("mkdir", [lock])).code !== 0) {
      // Another copy is starting one; a start that never finished (its copy closed) is
      // taken over.
      const age = await exec("stat", (await onMac()) ? ["-f", "%m", lock] : ["-c", "%Y", lock]);
      if (age.code === 0 && Date.now() / 1000 - Number(age.stdout) > START_MAX / 1000) {
        await exec("rmdir", [lock]);
      }
      await delay(cx, 50);
      continue;
    }
    try {
      // Started meanwhile, by the copy that had the lock before.
      const now = await readTextFile(file).catch(() => null);
      const started = parseRunFile(now);
      if (usable(started)) return found(started);
      // A newer host (or one that runs this module) takes over: the one running keeps
      // its connections and exits once they're gone.
      const got = await startHost(cx, best);
      const line = `${got.port} ${got.token} ${best.version}\n`;
      const wrote = await readTextFile(file).catch(() => null);
      if (wrote !== line) await writeTextFile(file, line, wrote ?? null);
      return { ...got, version: best.version, protocol: best.protocol };
    } finally {
      await exec("rmdir", [lock]);
    }
  }
  throw new Error("wasm-mods-host is still starting (in another pane): try again");
}

/** The host at `port` went away: forgotten, here and in HOST_FILE (unless another copy
 *  has written a new one). */
async function forgetHost(port) {
  const h = hostState();
  if (h.port === port) Object.assign(h, { port: 0, token: "", version: "", protocol: null });
  const file = HOST_FILE();
  const text = await readTextFile(file).catch(() => null);
  if (text?.startsWith(`${port} `)) await writeTextFile(file, "", text).catch(() => {});
}

async function startHost(cx, host) {
  const token = randomBytes(24).toString("hex");
  const id = randomBytes(6).toString("hex");
  const dir = RUN_DIR();
  const tokenFile = `${dir}/token-${id}`, portFile = `${dir}/port-${id}`;
  await writeTextFile(tokenFile, token, null);
  // No labels: herdr-app doesn't show it. --rm: gone once it exits (by itself, idle).
  const r = await hc(["run", "--bg", "--json", "--no-parent", "--rm", "--",
    host.path, "--token-file", tokenFile, "--port-file", portFile, "--host-file", HOST_FILE()]);
  if (r.code !== 0) {
    exec("rm", ["-f", tokenFile]);
    throw new Error(`can't start wasm-mods-host: ${r.stderr || r.stdout}`);
  }
  const terminal = JSON.parse(r.stdout).address;
  for (let i = 0; i < 300; i++) {
    const text = await readTextFile(portFile);
    if (text && /^\d+\n$/.test(text)) {
      exec("rm", ["-f", portFile]);
      return { port: Number(text), token };
    }
    await delay(cx, i < 50 ? 10 : 50);
  }
  const screen = await hc(["read", terminal, "--lines", "10"]);
  throw new Error(`wasm-mods-host didn't start: ${screen.stdout.trim() || "no output"}`);
}

/** What the "core needed" panel offers, for mod folder `folder`: {lock, target, bundled,
 *  url, canBuild, error}. */
export async function coreOffer(folder) {
  const out = { lock: null, target: null, url: null, canBuild: false, error: null };
  try {
    out.lock = await readLock(folder);
    out.target = await target();
    if (!out.lock) out.error = "this mod has no host.lock.json: reinstall it";
    else if (!out.target) out.error = "WASM Mods runs on Linux (x86_64, aarch64) for now";
    else if (!out.lock.host.files[out.target]) out.error = `no host for ${out.target} in host.lock.json`;
    else out.url = await hostUrl(out.lock, out.target);
    out.canBuild = Boolean(out.lock?.host.source) && (await canBuild());
  } catch (e) {
    out.error = String(e?.message ?? e);
  }
  return out;
}

/** Gets a host that runs `protocol` installed, for mod folder `folder`: the bundled one if
 *  it's there and checks out; otherwise CoreNeeded (the user picks how). */
/** Puts the module and a host in place; the name the host knows the module by. */
async function prepareCore(folder, module, protocol, core) {
  const name = module && !core ? await installModule(folder, module) : module;
  const h = hostState();
  if (h.port && h.protocol && fits(h, protocol)) return name;
  if (pickHost(await installedHosts(), protocol)) return name;
  const lock = await readLock(folder);
  const tgt = await target();
  if (lock && tgt && fits(lock.host, protocol)) await installBundled(folder, lock, tgt);
  return name;
}

// ---------------------------------------------------------------------------
// Colors and the theme
// ---------------------------------------------------------------------------

/** For an event of which only the latest counts (a pointer move or drag, a view): what
 *  it replaces is keyed by this; else null. */
function latestOnly(e) {
  if (e.Pointer && (e.Pointer.kind === "Move" || e.Pointer.kind === "Drag")) return `p:${e.Pointer.id}:${e.Pointer.kind}`;
  if (e.View) return `v:${e.View.id}`;
  return null;
}

/** "#rrggbb[aa]" → 0xRRGGBBAA. */
function rgba(hex) {
  const h = String(hex ?? "").replace("#", "");
  const n = parseInt(h.length === 6 ? h + "ff" : h.slice(0, 8), 16);
  return Number.isFinite(n) ? n >>> 0 : 0xff00ffff;
}

/** Where this copy's tab works, as the protocol's WindowContext: the folder it was
 *  opened for (props.folder: a diff opened from the git graph), else the tab's first
 *  terminal's, else its space's; null when there's none. */
function protocolWindow(p) {
  const all = panes();
  const tab = all.find((x) => x.pane === p.pane)?.tab ?? focused()?.tab;
  const space = spaces().find((s) => s.tabs.some((t) => t.id === tab));
  const machine = space?.machine ?? null;
  if (p.folder) return { folder: String(p.folder), branch: null, machine };
  const term = all.find((x) => x.tab === tab && x.terminal && x.cwd);
  const folder = term?.cwd ?? (space ? space.folder || "~" : null);
  return folder ? { folder, branch: term?.branch ?? space?.branch ?? null, machine } : null;
}

/** Whether a node is or holds a virtual list ([`ListWindow`]); outside popups. */
function holdsVirtual(n) {
  const [type, v] = Object.entries(n ?? {})[0] ?? [];
  if (type === "List") return Boolean(v.window);
  if (type === "Column" || type === "Row") return (v.children ?? []).some(holdsVirtual);
  return false;
}

/** 0xRRGGBBAA → "#rrggbbaa". */
function hex(n) {
  return "#" + (n >>> 0).toString(16).padStart(8, "0");
}

/** herdr-app's theme as the protocol's. */
function protocolTheme(t) {
  return {
    name: String(settings()?.theme ?? "herdr"),
    space: rgba(t.background), surface: rgba(t.surface), text: rgba(t.foreground),
    subtext: rgba(t.muted), muted: rgba(t.dim), accent: rgba(t.accent), border: rgba(t.border),
    ansi: Array.from({ length: 16 }, (_, i) => rgba(t[`ansi${i}`])),
    fonts: { ui: "", mono: String(settings()?.font_family ?? "") },
  };
}

/** A protocol Paint as a color. */
function paint(p, t) {
  if (!p) return null;
  if (p.Rgba !== undefined) return hex(p.Rgba);
  return roleColor(p.Role, t);
}

function roleColor(role, t) {
  switch (role) {
    case "Text": case "Heading": return t.foreground;
    case "Subtext": return t.muted;
    case "Muted": return t.dim;
    case "Accent": return t.accent;
    case "Danger": return t.ansi1;
    case "Success": return t.ansi2;
    case "Warning": return t.ansi3;
    case "Info": return t.ansi4;
    case "Space": return t.background;
    case "Surface": return t.surface;
    case "Border": case "Divider": return t.border;
    default: return t.foreground;
  }
}

const PAD = { None: 0, Xs: 4, S: 6, M: 10, L: 14 };

/** Unit circles as polygons, for small and larger dots. */
const circle = (n) => Array.from({ length: n }, (_, i) => [Math.cos((2 * Math.PI * i) / n), Math.sin((2 * Math.PI * i) / n)]);
const CIRCLE4 = circle(4), CIRCLE12 = circle(12), CIRCLE32 = circle(32);

const ACTION_GLYPH = { Back: "esc", Open: "⏎", Primary: "space", Secondary: "tab" };

/** `v` as JSON.stringify sends it: keys whose value is undefined left out (the app's
 *  socket takes values, not text, and would get them as null, which the wire refuses). */
function defined(v) {
  if (Array.isArray(v)) return v.map(defined);
  if (!v || typeof v !== "object") return v;
  const out = {};
  for (const k in v) if (v[k] !== undefined) out[k] = defined(v[k]);
  return out;
}

/** A frame's text fields (as they are) and lists ({id, selected, count, keyboard,
 *  virtual}), in tree order: what `adopt` needs (the app's socket finds them itself). */
function findFieldsAndLists(ui) {
  const fields = [], lists = [];
  const walk = (n) => {
    if (!n || typeof n !== "object") return;
    const [type, v] = Object.entries(n)[0] ?? [];
    if (type === "TextField") {
      fields.push(v);
    } else if (type === "List") {
      const virtual = Boolean(v.window);
      lists.push({ id: v.id, selected: v.selected, count: virtual ? Number(v.window.total) || 0 : v.items.length, keyboard: Boolean(v.keyboard), virtual, pages: virtual && Boolean(v.pages) });
      v.items.forEach((it) => walk(it.child));
    } else if (type === "Column" || type === "Row") {
      v.children.forEach(walk);
    } else if (type === "Popup") {
      walk(v.child);
    }
  };
  walk(ui);
  return { fields, lists };
}

// ---------------------------------------------------------------------------
// The view
// ---------------------------------------------------------------------------

/** The mod folder this file was loaded from, as herdr-app named it: a folder renamed when
 *  it was added (its name was taken) still finds its own files. herdr-app gives modules no
 *  import.meta.url; a stack trace names the file. Null if it can't tell (main.js's name
 *  is the fallback). */
export function folderOf(stack) {
  const m = /\/mods\/([^/\s?#()]+)\/wasm-bridge\.js/.exec(String(stack ?? ""));
  return m ? m[1] : null;
}
const OWN_FOLDER = folderOf(new Error().stack);

/** This copy's mod folder (each copy runs its own evaluation of this file). */
let FOLDER = OWN_FOLDER ?? "wasm-mods";

/** The view of a WASM mod folder's module: `wasmMod({folder, module})` in its main.js
 *  (`folder` is used only if this file can't tell where it was loaded from; `core: true`
 *  for the core mod, which runs any installed module by name). */
export function wasmMod(config) {
  return class extends WasmView {
    static config = config;
  };
}

export class WasmView extends View {
  init(_props, cx) {
    // Once: a second init would open a second connection.
    if (this.inited) return;
    this.inited = true;
    bindTab(cx);
    // This copy's props, read here once: `props()` after an await may answer for
    // another mod's scope (another copy's: a diff opened for a commit showed the
    // uncommitted changes, having read a copy's props without its arg).
    const p = this.props = props() ?? {};
    const config = this.constructor.config ?? {};
    this.folder = FOLDER = OWN_FOLDER ?? config.folder ?? "wasm-mods";
    this.core = Boolean(config.core);
    this.module = String(config.module ?? p.module ?? "");
    this.need = null;       // no host runs the module: what the panel offers
    this.installing = null; // what the panel is doing (a download, a build)
    this.status = this.module ? "starting…" : "no module: give this mod props {module: \"<name>\"}";
    this.out = null;        // the last frame: {title, ui, actions}
    this.error = null;
    this.size = null;
    this.sentTheme = "";
    this.seq = 0;
    this.sent = new Map();  // seq → when it was sent (round trips)
    this.fields = new Map(); // text field id → {state, revision, focused}
    this.lists = new Map();  // list id → {selected, count, keyboard}
    this.virtual = new Map(); // virtual list id → {handle, sent, revision}
    this.keyList = null;     // the list the keys move
    this.fieldFocused = false;
    this.canvases = new Map(); // canvas id → {w, h, view, moved, reset, down}
    this.focus = cx.focus_handle();
    if (this.module && NATIVE_VIEW) this.watchNative(cx);
    if (this.module) cx.spawn(async (cx) => this.connect(cx));
  }

  async close() {
    this.closed = true;
    this.stopNativeTick();
    this.sock?.close();
  }

  // ----- the app's own view (only where `herdr.wasmView()` says it has one)

  /** The native view's state is made from a timer: it runs in this copy's own scope,
   *  while what follows an await may run in another mod's (a menu closing as this
   *  opens), which would own the state and drop it with itself. The timer then watches
   *  the view's connection: a lost host is found again (or another started) here, where
   *  hosts are known, and the view connected to it. "focus" from the app gives the keys
   *  to the view. Props `native: false` keep this script's drawing. */
  watchNative(cx) {
    if (this.props.native === false) return;
    this.nativeTick = cx.timer.every(20, (cx) => {
      if (this.pendingNative) {
        const make = this.pendingNative;
        this.pendingNative = null;
        make();
        cx.notify();
      }
      const state = this.native?.state;
      if (!state) return;
      if (this.pendingFocus) {
        this.pendingFocus = false;
        if (typeof state.focus === "function") state.focus();
      }
      if (this.pendingConnect) {
        const { port, token } = this.pendingConnect;
        this.pendingConnect = null;
        state.connect(port, token);
        return;
      }
      // Looked at every 100 ms or so; nothing to do while it's connected.
      if (this.findingHost || (this.lostTicks = ((this.lostTicks ?? 0) + 1) % 5) !== 0) return;
      const lost = state.lost();
      if (!lost) return;
      this.findingHost = true;
      cx.spawn(async (cx) => {
        try {
          this.pendingConnect = await this.hostAgain(cx, lost);
        } catch (e) {
          console.warn(`wasm-mods: ${this.module}: ${e?.message ?? e}`);
          await delay(cx, 1000);
        } finally {
          this.findingHost = false;
        }
      });
    });
    if (typeof herdr.nextCommand === "function") {
      cx.spawn(async (cx) => {
        while (!this.closed) {
          const command = await herdr.nextCommand();
          if (this.closed) return;
          if (command !== "focus") continue;
          if (this.native || this.pendingNative) this.pendingFocus = true;
          else this.focus.focus();
          cx.notify();
        }
      });
    }
  }

  /** Hands the pane to the app's view, which connects to the host and draws the module
   *  itself. False when there's none (this script draws). */
  async goNative(cx, port, token) {
    if (!this.nativeTick) return false;
    const native = await import("herdr-components").catch(() => null);
    if (!native?.WasmViewState || !native.WasmView) {
      this.stopNativeTick();
      return false;
    }
    const p = this.props;
    const json = JSON.stringify({
      module: this.hostName ?? this.module, arg: p.arg ?? null, open: p.open ?? null,
      port, token, images: IMAGE_DIR(), pane: p.pane ?? null, folder: p.folder ?? null,
    });
    // Made on the next tick (see watchNative).
    this.pendingNative = () => {
      this.native = { WasmView: native.WasmView, state: native.WasmViewState(json) };
      this.status = null;
    };
    return true;
  }

  stopNativeTick() {
    this.nativeTick?.cancel();
    this.nativeTick = null;
  }

  /** The native view lost its host ({port, why}): where to connect it now. A host that
   *  can't be reached is forgotten (and another started); one that only closed this
   *  connection is tried again first, as the connection here would be. */
  async hostAgain(cx, lost) {
    await delay(cx, 300);
    if (/can't reach/.test(lost.why)) await forgetHost(lost.port);
    return ensureHost(cx, this.protocol, this.folder);
  }

  // ----- the connection

  /** The protocol this copy's module needs (its lock says; the core mod's modules: the
   *  newest its lock's host runs), with the module and a host installed if the folder
   *  has them. */
  /** The protocol this module speaks, after its .wasm and a host are in place. */
  async prepareModule() {
    const lock = await readLock(this.folder).catch(() => null);
    const protocol = lock?.module?.protocol ?? lock?.host.protocol[1] ?? 4;
    this.hostName = await prepareCore(this.folder, this.module, protocol, this.core);
    return protocol;
  }

  async connect(cx, tries = 0) {
    try {
      this.protocol ??= await this.prepareModule();
      let got;
      try {
        got = await ensureHost(cx, this.protocol, this.folder);
      } catch (e) {
        if (!(e instanceof CoreNeeded)) throw e;
        this.need = await coreOffer(this.folder);
        this.status = null;
        cx.notify();
        // Another copy (or the user, by hand) may install one meanwhile: then go on.
        while (this.need && !this.closed) {
          await delay(cx, 3000);
          if (this.installing || !this.need) continue;
          hostsChanged();
          if (pickHost(await installedHosts(), this.protocol)) {
            this.need = null;
            return this.connect(cx);
          }
        }
        return;
      }
      const { port, token } = got;
      if (tries === 0 && (await this.goNative(cx, port, token))) return;
      try {
        this.sock = await net.connect("127.0.0.1", port);
        this.port = port;
        this.error = null;
      } catch (e) {
        // A host that went (idle, or killed): start another.
        if (tries >= 3) throw e;
        await forgetHost(port);
        await delay(cx, 100);
        return this.connect(cx, tries + 1);
      }
      const p = this.props;
      // Canvases come as their shapes, drawn here as a few native paths, never as a
      // picture the host drew: herdr-app keeps every picture file it has shown until it
      // quits, and a big canvas changing (a stroke, a pan) would be a new one each time.
      this.send({ hello: { token, module: this.hostName ?? this.module, arg: p.arg ?? null, open: p.open ?? null,
        images: IMAGE_DIR(), native_canvas: true } });
      // A new connection knows nothing yet: the theme too (the host draws nothing
      // without one), even if the last connection had it.
      this.sentTheme = "";
      this.sendTheme();
      this.sentWindow = "";
      this.sendWindow();
      this.sentFocused = null;
      this.sendFocused();
      if (this.size) this.send({ size: this.size });
      await this.readLoop(cx);
    } catch (e) {
      this.status = null;
      this.error = String(e?.message ?? e);
      cx.notify();
      return;
    }
    // The host closed this connection after saying why (the module isn't installed, or
    // didn't load): the reason stays; trying again would only fail the same way.
    if (!this.closed && this.error) {
      this.status = null;
      cx.notify();
      return;
    }
    if (!this.closed) {
      // The connection went: connect again. A host that's gone fails to connect, and
      // only then is it forgotten and another started (the catch above); one that's
      // still there keeps serving this app's other panes.
      this.status = "reconnecting…";
      cx.notify();
      await delay(cx, 500);
      if (!this.closed) return this.connect(cx);
    }
  }

  send(msg) {
    this.sock?.write(JSON.stringify(msg) + "\n").catch(() => {});
  }

  connected() {
    return Boolean(this.sock);
  }

  sendWindow() {
    if (!this.connected()) return;
    const json = JSON.stringify(protocolWindow(this.props));
    if (json === this.sentWindow) return;
    this.sentWindow = json;
    this.send({ window: JSON.parse(json) });
  }

  /** Whether this copy's pane has the keyboard (Input.focused): Herdr's focused pane, or
   *  (outside a pane of its own) this view's focus. Sent when it changes. */
  sendFocused() {
    if (!this.connected()) return;
    const pane = this.props.pane;
    const now = pane ? focused()?.pane === pane : this.focus.is_focused();
    if (now === this.sentFocused) return;
    this.sentFocused = now;
    this.send({ focused: now });
  }

  sendTheme() {
    const json = JSON.stringify(protocolTheme(theme()));
    if (json === this.sentTheme || !this.connected()) return;
    this.sentTheme = json;
    this.send({ theme: JSON.parse(json) });
  }

  /** Events in the protocol's form; `seq` times the round trip. `m`: GPUI modifiers. */
  sendEvents(events, m) {
    if (!events.length && !this.held?.size) return;
    // Input here means its pane has the keyboard: say so first.
    this.sendFocused();
    // Moves and views while the module hasn't answered yet: only the latest of each is
    // sent, with its answer (the app would otherwise draw a frame for every one, falling
    // ever further behind).
    const oldest = this.sent.values().next().value;
    if (events.length && events.every(latestOnly) && oldest !== undefined && Date.now() - oldest < 1000) {
      this.held ??= new Map();
      for (const e of events) this.held.set(latestOnly(e), { e, m });
      return;
    }
    if (this.held?.size) {
      const held = [...this.held.values()];
      this.held = null;
      events = [...held.map((h) => h.e), ...events];
      m ??= held[held.length - 1].m;
    }
    const seq = ++this.seq;
    this.sent.set(seq, Date.now());
    const mods = m ? { ctrl: Boolean(m.control), shift: Boolean(m.shift), alt: Boolean(m.alt) } : undefined;
    this.send({ events: { seq, events, mods } });
  }

  async readLoop(cx) {
    // This connection's socket, even if a reconnect replaces this.sock meanwhile: two
    // loops reading one socket would each get part of a frame.
    const sock = this.sock;
    let buf = new Uint8Array(0);
    // A closed copy stops here (its timers may no longer wait: a retry would spin).
    while (!this.closed) {
      let chunk;
      try {
        chunk = await sock.read(1 << 16);
      } catch (e) {
        if (this.closed) return;
        // The runtime's read can give up with "try again" (EAGAIN) instead of waiting.
        if (/temporarily unavailable|os error 11|would block/i.test(String(e?.message ?? e))) {
          await delay(cx, 5);
          continue;
        }
        console.warn(`wasm-mods: ${e}; reconnecting`);
        return;
      }
      if (chunk === null || this.closed) return;
      const joined = new Uint8Array(buf.length + chunk.length);
      joined.set(buf);
      joined.set(chunk, buf.length);
      buf = joined;
      let nl;
      while ((nl = buf.indexOf(10)) >= 0) {
        const line = Buffer.from(buf.subarray(0, nl)).toString("utf8");
        buf = buf.slice(nl + 1);
        try { this.receive(JSON.parse(line), cx); } catch (e) { console.warn(`wasm-mods: ${e}`); }
      }
      cx.notify();
    }
  }

  /** `found`: the frame's text fields and lists, when the app found them. */
  receive(msg, cx, found) {
    if (msg === "close") {
      const pane = this.props.pane;
      if (pane) run("pane.close", { pane });
      return;
    }
    if (msg.ready) {
      this.status = null;
    } else if (msg.frame) {
      this.out = msg.frame;
      this.error = null;
      this.acked(msg.frame.ack);
      this.adopt(msg.frame.ui, cx, found);
      this.prepareCanvases(msg.frame.ui);
    } else if (msg.error) {
      this.error = msg.error.message;
    } else if (msg.focus) {
      this.fields.get(msg.focus.id)?.state.focus();
    } else if (msg.open) {
      // A module opened from this one (the git graph's diff), in a new tab of this
      // space, working in the same folder.
      const tab = panes().find((x) => x.pane === this.props.pane)?.tab ?? focused()?.tab;
      const space = spaces().find((s) => s.tabs.some((t) => t.id === tab))?.id;
      const folder = protocolWindow(this.props)?.folder;
      const extra = { arg: msg.open.arg, ...(folder ? { folder } : {}) };
      // Its own mod folder if it has one (wasm-diff), else the core mod runs it.
      const own = `wasm-${msg.open.module.replace(/_/g, "-")}`;
      exec("test", ["-f", `${MOD_DIR(own)}/mod.json`]).then((r) => r.code === 0).catch(() => false).then((has) => {
        const name = msg.open.module.replace(/_/g, " ");
        run("tab.new", has ? { space, name, mod: own, props: extra } : { space, name, mod: "wasm-mods", props: { module: msg.open.module, ...extra } });
      });
    } else if (msg.open_url) {
      openWithDesktop(msg.open_url.url);
    }
  }

  /** An answer covers every event sent up to its ack. */
  acked(ack) {
    for (const seq of this.sent.keys()) if (seq <= ack) this.sent.delete(seq);
    if (!this.sent.size && this.held?.size) this.sendEvents([]);
  }

  /** Text fields and lists in the new tree: states for new fields (made here, in a
   *  live call, never in render), new text when the module raised a revision. `found`:
   *  the tree's fields and lists as the app found them (else this walks the tree). */
  adopt(ui, cx, found) {
    const { fields, lists } = found ?? findFieldsAndLists(ui);
    const seen = new Set();
    let firstKeyList = null;
    for (const v of fields) {
      seen.add(v.id);
      let f = this.fields.get(v.id);
      if (!f) {
        const state = InputState.new({ placeholder: v.placeholder ?? "", value: v.value ?? "" });
        f = { state, revision: v.revision ?? 0, focused: false };
        state.on("change", (_e, cx) => {
          this.sendEvents([{ Edited: { id: v.id, value: state.value(), revision: f.revision } }]);
        });
        state.on("submit", (_e, cx) => this.sendEvents([{ Submitted: { id: v.id } }]));
        state.on("focus", (_e, cx) => { f.focused = this.fieldFocused = true; this.sendEvents([{ Focus: { id: v.id, focused: true } }]); cx.notify(); });
        state.on("blur", (_e, cx) => { f.focused = this.fieldFocused = false; this.sendEvents([{ Focus: { id: v.id, focused: false } }]); cx.notify(); });
        this.fields.set(v.id, f);
      } else if ((v.revision ?? 0) !== f.revision) {
        // Up: the module set the text. Down: a new session (a reconnect, the host
        // restarted) whose module counts from the start again; edits must carry its
        // revision or it drops them all.
        f.revision = v.revision ?? 0;
        f.state.set_value(v.value ?? "");
      }
      f.state.set_masked(Boolean(v.password));
    }
    for (const v of lists) {
      // A selection made here and not answered yet stays (no flicker back while the
      // module's answer is on its way).
      const mine = this.lists.get(v.id);
      const pending = this.sent.size > 0 && mine?.local;
      this.lists.set(v.id, { selected: pending ? mine.selected : v.selected, count: v.count, local: pending });
      if (v.keyboard && firstKeyList === null) firstKeyList = v.id;
      // A virtual list's scroll handle (made here: not allowed while drawing).
      if (v.virtual && !this.virtual.has(v.id)) {
        this.virtual.set(v.id, { handle: VirtualListScrollHandle.new(), sent: "", revision: -1 });
      }
    }
    for (const id of [...this.fields.keys()]) if (!seen.has(id)) this.fields.delete(id);
    // A list gone from the frame (another module shown in this window, the diff over
    // the graph) no longer takes keys. Its scroll handle stays: back, the list is
    // where it was.
    const listed = new Set(lists.map((v) => v.id));
    // The module's page (Node::List pages): PgUp/PgDn and Home/End scroll it.
    this.pageList = lists.find((v) => v.pages)?.id ?? null;
    for (const id of [...this.lists.keys()]) if (!listed.has(id)) this.lists.delete(id);
    if (!this.keyList || !this.lists.has(this.keyList)) this.keyList = firstKeyList;
  }

  // ----- keys: actions, never raw keys

  /** Tab (Shift+Tab: `back`): the module's Secondary action, Shift in its mods; in a
   *  text field, the keyboard moves on to the next field as the shell's Tab would. */
  tab(back, cx) {
    if (this.inField()) {
      const w = globalThis.window;
      if (back) w?.focus_prev?.(); else w?.focus_next?.();
      return;
    }
    this.sendEvents([{ Action: "Secondary" }], { shift: back });
    cx.notify();
  }

  /** Scrolls the module's page list a page up or down, or to its start or end; false if
   *  the module has none. The module hears where it is (VisibleRange). */
  pageBy(move) {
    const state = this.pageList != null && this.virtual.get(this.pageList);
    if (!state) return false;
    const total = this.lists.get(this.pageList)?.count ?? 0;
    const [first, count] = (state.sent || "0:0").split(":").map(Number);
    // The range counts the rows partly showing at both ends: a page keeps a row or so
    // of the last one. The row in the middle moves by a page ("center" puts it there;
    // "top" would only bring it to the nearer edge).
    const page = Math.max(1, count - 3);
    const mid = first + Math.floor(count / 2);
    const to = { PageUp: mid - page, PageDown: mid + page, Home: 0 }[move];
    if (move === "End") state.handle.scroll_to_bottom();
    else state.handle.scroll_to_item(Math.max(0, Math.min(total - 1, to)), move === "Home" ? "top" : "center");
    return true;
  }

  /** Whether a text field has the keyboard. The field's focus and blur events don't
   *  come on every herdr-app (0.1.11 sends neither), so a key that reaches this view's
   *  root while the root itself doesn't hold the keyboard means a field does: only text
   *  fields take it inside the view. */
  inField() {
    return this.fieldFocused || (this.fields.size > 0 && !this.focus.is_focused());
  }

  key(e, cx) {
    const inField = this.inField();
    if (inField && !["escape", "tab"].includes(e.key)) return;
    const m = e.modifiers ?? {};
    if (m.control || m.alt || m.platform) return;
    const list = this.keyList && this.lists.get(this.keyList);
    const move = { up: "Up", down: "Down", pageup: "PageUp", pagedown: "PageDown", home: "Home", end: "End", left: "Left", right: "Right" }[e.key]
      ?? (!inField && e.key === "j" ? "Down" : !inField && e.key === "k" ? "Up" : null);
    let events = [];
    // A key the module offers by name ({action: {Key: name}, key: "s"}): that action.
    const named = !inField && !m.shift
      ? this.out?.actions?.find((o) => o.action?.Key != null && o.key === e.key) : null;
    if (named) {
      events = [{ Action: { Key: named.action.Key } }];
    } else if (["PageUp", "PageDown", "Home", "End"].includes(move) && this.pageBy(move)) {
      cx.notify();
    } else if (move && list && list.count > 0 && !["Left", "Right"].includes(move)) {
      const cur = list.selected ?? -1;
      const step = { Up: -1, Down: 1, PageUp: -10, PageDown: 10, Home: -1e9, End: 1e9 }[move];
      const next = Math.max(0, Math.min(list.count - 1, cur + step));
      Object.assign(list, { selected: next, local: true });
      // A virtual list scrolls to it (the others are scrolled by their container).
      this.virtual.get(this.keyList)?.handle.scroll_to_item(next);
      events = [{ Selected: { id: this.keyList, index: next } }];
      cx.notify();
    } else if (move) {
      events = [{ Action: { Move: move } }];
    } else if (e.key === "enter" && list && list.selected != null) {
      events = [{ Activated: { id: this.keyList, index: list.selected } }];
    } else if (e.key === "escape") {
      const popup = this.openPopup;
      events = [popup ? { Dismissed: { id: popup } } : { Action: "Back" }];
      if (inField) this.focus.focus();
    } else if (e.key === "enter") {
      events = [{ Action: "Open" }];
    } else if (e.key === "space") {
      events = [{ Action: "Primary" }];
    } else if (e.key === "tab" && !inField) {
      events = [{ Action: "Secondary" }];
    }
    if (events.length) {
      this.sendEvents(events);
      cx.stop_propagation();
    }
  }

  // ----- drawing

  /** "WASM Mods core needed": no installed host runs this module. Says what would be
   *  installed and from where; nothing is fetched or built without a click. */
  corePanel(root, t) {
    const n = this.need;
    const lock = n.lock, host = lock?.host, file = host && n.target ? host.files[n.target] : null;
    const mb = file?.size ? ` ${(file.size / 1048576).toFixed(1)} MB,` : "";
    const busy = Boolean(this.installing);
    const button = (id, label, primary, action) => div().id(id).px(12).py(5).rounded(7).border_1().border_color(t.border)
      .bg(primary ? t.accent : t.raised).text_color(primary ? t.accent_foreground : t.foreground).text_size(13)
      .when(!busy, (el) => el.cursor_pointer().hover((s) => s.opacity(0.9)).on_click((_e, cx) => this.installCore(cx, action)))
      .when(busy, (el) => el.opacity(0.5))
      .child(label);
    const line = (text, color = t.muted) => div().text_size(12.5).line_height("18px").text_color(color).child(text);
    const mono = (text) => div().text_size(12).text_color(t.dim).font_family(String(settings()?.font_family || "monospace")).child(text);
    const panel = v_flex().id("wasm-core-needed").gap(8).p(16).max_w(620)
      .child(div().text_size(15).font_semibold().text_color(t.foreground).child("WASM Mods core needed"))
      .child(line(`${this.module || "This mod"} runs in wasm-mods-host, the small program that keeps WASM mods sandboxed. No host installed here runs it (protocol ${this.protocol}).`));
    if (host) {
      panel.child(line(`Installs wasm-mods-host ${host.version} (${protocols(host.protocol)}) into ~/.local/share/wasm-mods/host/${host.version}/, beside any other version. Nothing is removed.`));
      if (n.url) panel.child(line("From:")).child(mono(n.url)).child(mono(`sha256 ${file.sha256}`));
      if (host.source && n.canBuild) panel.child(line(`Or built here from ${host.source.git} (${host.source.tag ?? host.source.rev}) with cargo.`));
    }
    if (n.error) panel.child(line(n.error, t.ansi1));
    const buttons = h_flex().gap(8).pt(4);
    if (n.url && !n.error) buttons.child(button("wasm-core-download", `Download v${host.version} (${mb} checksum pinned)`.replace("( ", "("), true, "download"));
    if (host?.source && n.canBuild) buttons.child(button("wasm-core-build", "Build from source", !n.url, "build"));
    panel.child(buttons);
    if (this.installing) panel.child(line(this.installing));
    if (this.installError) panel.child(line(this.installError, t.ansi1));
    return root.child(div().size_full().flex().items_center().justify_center().child(panel));
  }

  /** The panel's buttons: `download` or `build`, then connect. */
  installCore(cx, how) {
    if (this.installing) return;
    const n = this.need;
    this.installing = how === "download" ? "Downloading…" : "Building… (a few minutes)";
    this.installError = null;
    cx.notify();
    cx.spawn(async (cx) => {
      try {
        if (how === "download") await downloadHost(n.lock, n.target);
        else await buildHost(cx, n.lock, (text) => { this.installing = text; cx.notify(); });
        hostsChanged();
        this.installing = null;
        this.need = null;
        this.status = "starting…";
        cx.notify();
        await this.connect(cx);
      } catch (e) {
        this.installing = null;
        this.installError = String(e?.message ?? e);
        cx.notify();
      }
    });
  }

  render(cx) {
    if (this.native) return div().size_full().child(new this.native.WasmView(this.native.state));
    const t = theme();
    this.sendTheme();
    this.sendWindow();
    this.sendFocused();
    const root = v_flex().id("wasm-mods").size_full().track_focus(this.focus).key_context(KEY_CONTEXT)
      .on_key_down((e, cx) => this.key(e, cx))
      .on_action("wasm-mods::tab", (_e, cx) => this.tab(false, cx))
      .on_action("wasm-mods::tab-back", (_e, cx) => this.tab(true, cx))
      .on_mouse_down("left", () => { if (!this.fieldFocused) this.focus.focus(); })
      .on_size_change((reported, cx) => {
        const z = uiZoom(), size = { width: reported.width / z, height: reported.height / z };
        if (this.size?.width === size.width && this.size?.height === size.height) return;
        this.size = size;
        this.send({ size: this.size });
      })
      .text_color(t.foreground).text_size(13);
    if (this.need) return this.corePanel(root, t);
    if (this.error) {
      return root.p(14).gap(6)
        .child(div().text_size(14).font_semibold().child(this.module || "wasm-mods"))
        .child(div().text_color(t.ansi1).child(this.error));
    }
    if (!this.out) return root.p(14).child(div().text_color(t.dim).child(this.status ?? "loading…"));
    this.openPopup = null;
    this.popups = [];
    let body;
    try {
      body = this.node(this.out.ui, t, { inRow: false });
    } catch (e) {
      // A tree it can't draw: say why rather than keep the last picture.
      console.warn(`wasm-mods: ${this.module}: ${e?.stack ?? e}`);
      return root.p(14).child(div().text_color(t.ansi1).child(`can't draw ${this.module}: ${e?.message ?? e}`));
    }
    // The frame scrolls, unless a virtual list in it does.
    let content = v_flex().id("wasm-mods-scroll").flex_1().min_h(0).p(14).child(body);
    // (What's wider than the pane is cut at its edge.)
    content = holdsVirtual(this.out.ui) ? content.overflow_hidden() : content.overflow_y_scroll();
    return root.relative().bg(t.background)
      .child(content)
      .when(this.out.actions?.length > 0, (el) => el.child(this.hints(t)))
      .children(this.popups);
  }

  /** What the user can do now: the module's offers, clickable. */
  hints(t) {
    return h_flex().id("wasm-mods-hints").flex_shrink_0().px(14).py(6).gap(14).border_t_1().border_color(t.border)
      .text_size(11.5).text_color(t.dim)
      .children(this.out.actions.map((o, i) => {
        const a = o.action;
        const glyph = typeof a === "string" ? ACTION_GLYPH[a] : a.Move ? "↑↓" : o.key ?? "";
        return h_flex().id(`hint-${i}`).gap(5).cursor_pointer().hover((s) => s.text_color(t.foreground))
          .on_click(() => this.sendEvents([{ Action: a }]))
          .child(div().px(5).rounded(4).bg(t.raised).text_color(t.muted).child(glyph))
          .child(o.label);
      }));
  }

  /** One node of the module's tree as GPUI elements. */
  node(n, t, ctx) {
    if (!n || n === "Empty") return div();
    const [type, v] = Object.entries(n)[0];
    switch (type) {
      case "Column": case "Row": return this.container(type, v, t, ctx);
      case "Text": return this.text(v, t);
      case "TextField": return this.field(v, t, ctx);
      case "List": return this.list(v, t, ctx);
      case "Canvas": return this.canvas(v, t, ctx);
      case "Popup": {
        this.openPopup = v.id;
        const panel = this.container("Column", { children: [v.child], gap: 0, layout: { padding: "M" }, style: { variant: "Panel", ...v.style } }, t, { inRow: false });
        this.popups.push(div().absolute().top(0).left(0).size_full().flex().items_center().justify_center()
          .when(v.modal, (el) => el.bg(t.background + "99"))
          .id(`popup-${v.id}`).on_click(() => { if (!v.modal) this.sendEvents([{ Dismissed: { id: v.id } }]); })
          .child(panel.id(`popup-panel-${v.id}`).bg(t.background).on_click((_e, cx) => cx.stop_propagation()).shadow_lg().max_w("90%")));
        return div();
      }
      default: return div();
    }
  }

  /** Width and height of a node from its Layout, in a row or a column. */
  sized(el, layout, ctx) {
    const w = layout?.width, h = layout?.height;
    // Filling part of a row: clipped to its share (side by side halves don't overlap).
    if (w === "Fill") el = ctx.inRow ? el.flex_1().min_w(0).overflow_hidden() : el.w_full();
    else if (w?.Fixed !== undefined) el = el.w(w.Fixed).flex_shrink_0();
    if (h === "Fill") el = ctx.inRow ? el.h_full() : el.flex_1().min_h(0);
    else if (h?.Fixed !== undefined) el = el.h(h.Fixed);
    return el;
  }

  container(type, v, t, ctx) {
    const row = type === "Row";
    const layout = v.layout ?? {}, style = v.style ?? {};
    let el = (row ? h_flex() : v_flex()).gap(v.gap ?? 0);
    if (v.id) el = el.id(`n-${v.id}`);
    // Look by variant.
    const variant = style.variant ?? "Plain";
    let pad = 0;
    if (variant === "Card") { el = el.bg(t.surface).border_1().border_color(t.border).rounded(8); pad = 10; }
    else if (variant === "Panel") { el = el.bg(t.popover).border_1().border_color(t.border).rounded(10); pad = 12; } // a popup's box: the bridge's own
    else if (variant === "Field") { el = el.bg(t.background).border_1().border_color(t.input).rounded(6); pad = 6; }
    else if (variant === "Pill") { el = el.bg(t.raised).rounded_full().text_size(11.5); pad = [2, 8]; }
    else if (variant === "Selected") { el = el.bg(t.selected).rounded(6); pad = 6; }
    else if (variant?.Button) {
      const kind = variant.Button;
      el = el.rounded(6).items_center().justify_center().text_size(12.5).font_medium();
      pad = [5, 12];
      if (kind === "Primary") el = el.bg(t.solid).text_color(t.solid_foreground).hover((s) => s.opacity(0.88));
      else if (kind === "Ghost") el = el.hover((s) => s.bg(t.hover));
      else if (kind === "Danger") el = el.border_1().border_color(t.ansi1).text_color(t.ansi1).hover((s) => s.bg(t.hover));
      else el = el.bg(t.raised).border_1().border_color(t.input).hover((s) => s.bg(t.hover));
    }
    const p = layout.padding && layout.padding !== "Auto" ? PAD[layout.padding] : pad;
    el = Array.isArray(p) ? el.py(p[0]).px(p[1]) : el.p(p);
    if (style.bg) el = el.bg(paint(style.bg, t));
    if (style.fg) el = el.text_color(paint(style.fg, t));
    if (style.border) el = el.border_1().border_color(paint(style.border, t));
    if (style.rounding) el = el.rounded({ None: 0, S: 4, M: 6, L: 10, Full: 999 }[style.rounding] ?? 6);
    if (style.opacity != null) el = el.opacity(style.opacity);
    // Layout.
    const align = layout.align ?? "Auto";
    if (align === "Center" || (align === "Auto" && row)) el = el.items_center();
    else if (align === "End") el = el.items_end();
    else if (align === "Start") el = el.items_start();
    const justify = layout.justify ?? "Start";
    if (justify === "Center") el = el.justify_center();
    else if (justify === "End") el = el.justify_end();
    else if (justify === "SpaceBetween") el = el.justify_between();
    if (row && layout.wrap !== false && justify === "Start") el = el.flex_wrap();
    if (layout.scroll && layout.scroll !== "None" && v.id) {
      el = layout.scroll === "Horizontal" ? el.overflow_x_scroll() : layout.scroll === "Both" ? el.overflow_scroll() : el.overflow_y_scroll();
    }
    el = this.sized(el, layout, ctx);
    // Holding a virtual list (which scrolls itself): it takes the height it's given.
    if ((!layout.height || layout.height === "Auto") && holdsVirtual({ [type]: v })) el = ctx.inRow ? el.h_full() : el.flex_1().min_h(0);
    if (v.id && !ctx.virtual) {
      const id = v.id;
      el = el.cursor_pointer().on_click((_e, cx) => { this.sendEvents([{ Clicked: { id } }]); cx.stop_propagation(); })
        .on_mouse_down("right", (e, cx) => {
          const p = unzoomed(e.local_position) ?? { x: 0, y: 0 };
          this.sendEvents([{ Secondary: { id, x: p.x, y: p.y } }]);
          cx.stop_propagation();
        });
    }
    if (v.tooltip) {
      const text = this.plain(v.tooltip);
      if (text) el = el.tooltip(text);
    }
    const kids = v.children ?? [];
    return el.children(kids.map((c) => this.node(c, t, { inRow: row, virtual: ctx.virtual })));
  }

  /** A node's text, for tooltips. */
  plain(n) {
    if (!n || typeof n !== "object") return "";
    const [type, v] = Object.entries(n)[0];
    if (type === "Text") return v.spans?.length ? v.spans.map((s) => s.text).join("") : v.text;
    if (type === "Column") return (v.children ?? []).map((c) => this.plain(c)).filter(Boolean).join("\n");
    if (type === "Row") return (v.children ?? []).map((c) => this.plain(c)).filter(Boolean).join(" ");
    return "";
  }

  text(v, t) {
    const style = v.style ?? "Body";
    let el = div();
    // Body, headings and titles take their container's color (a button's label).
    let size = 13, color = null;
    if (style === "Heading") { size = 15; el = el.font_semibold(); }
    else if (style === "Title") { size = 18; el = el.font_semibold(); }
    else if (style === "Muted") color = t.muted;
    else if (style === "Accent") color = t.accent;
    else if (style === "Small") { size = 11; color = t.dim; }
    else if (style === "Mono") size = 12.5;
    if ((style === "Mono" || v.font === "Mono") && settings()?.font_family) el = el.font_family(String(settings().font_family));
    if (v.size) size = v.size;
    if (v.bold) el = el.font_semibold();
    const c = paint(v.color, t);
    if (c) color = c;
    el = el.text_size(size);
    if (color) el = el.text_color(color);
    if (v.wrap === "Extend") el = el.whitespace_nowrap().flex_shrink_0();
    else if (v.wrap === "Truncate") el = el.w_full().min_w(0).truncate();
    if (v.spans?.length) {
      // On one line when it extends (a diff line), else wrapping between spans.
      let line = v.wrap === "Extend" ? h_flex().whitespace_nowrap().flex_shrink_0()
        : v.wrap === "Truncate" ? h_flex().w_full().min_w(0).overflow_hidden().whitespace_nowrap() : h_flex().flex_wrap();
      return line.text_size(size).children(v.spans.map((s, i) => {
        let span = div().child(s.text);
        if (s.color != null || color) span = span.text_color(s.color != null ? hex(s.color) : color);
        if (s.bg != null) span = span.bg(hex(s.bg)).rounded(3);
        if (s.link && !ctx?.virtual) {
          const target = s.link;
          span = span.id(`link-${i}-${target}`).cursor_pointer().hover((st) => st.underline()).tooltip(target)
            .on_click(() => { if (/^(https?:\/\/|file:\/\/|\/|~\/)/.test(target)) openWithDesktop(target.replace(/^~\//, `${home()}/`)); });
        }
        return span;
      }));
    }
    return el.child(v.text ?? "");
  }

  field(v, t, ctx) {
    const f = this.fields.get(v.id);
    if (!f) return div();
    const mono = v.monospace !== false && settings()?.font_family;
    let el = Input.new(f.state).h(32).px(8).rounded(6).border_1()
      .border_color(f.focused ? t.accent : t.input).bg(t.background).text_size(12.8).text_color(t.foreground);
    if (mono) el = el.font_family(String(settings().font_family));
    // The press is the field's: the pane's own focus (for keys) mustn't take it back.
    const wrap = div().flex().on_mouse_down("left", (_e, cx) => cx.stop_propagation()).child(el.w_full());
    return this.sized(wrap, v.layout, ctx);
  }

  list(v, t, ctx) {
    if (v.window) return this.virtualList(v, t, ctx);
    const rows = v.items.map((it, i) => this.listRow(v, it, i, t));
    let el = v_flex().id(`list-${v.id}`).gap(1).children(rows);
    if (v.font === "Mono" && settings()?.font_family) el = el.font_family(String(settings().font_family));
    el = this.sized(el, v.layout, ctx);
    // A height of its own: rows past it scroll, inside it (never drawn over what's below).
    if (v.layout?.height?.Fixed !== undefined || v.layout?.height === "Fill") el = el.overflow_y_scroll();
    if (!v.layout?.width || v.layout.width === "Auto") el = el.w_full();
    return el;
  }

  /** A list of `window.total` rows of which the frame sent `items` (rows `window.first..`):
   *  only the rows on screen are built, the module hears which those are
   *  (VisibleRange) and sends them for the next frame; rows it hasn't sent yet are
   *  blank. It fills the height it's given (all that's left, unless fixed). */
  virtualList(v, t, ctx) {
    const w = v.window, total = Number(w.total) || 0, first = Number(w.first) || 0;
    const state = this.virtual.get(v.id);
    if (w.scroll_to && w.scroll_to.revision !== state.revision) {
      state.revision = w.scroll_to.revision;
      const row = Math.min(Number(w.scroll_to.row), Math.max(0, total - 1));
      state.handle.scroll_to_item(row, "top");
    }
    // One height for every row: the list places rows by it, in the script's pixels (the
    // app zooms a list's item sizes, as it does the row). Code (dense, monospace: a
    // diff's lines) sits tight, line under line.
    const rowH = v.dense ? (v.font === "Mono" ? 20 : 26) : 36;
    let el = v_virtual_list(`list-${v.id}`, total, rowH, (i) => String(i), (range) => {
      // (A one-row range is the list measuring itself, not what shows.)
      const seen = `${range.start}:${range.end - range.start}`;
      if (seen !== state.sent && (range.end - range.start > 1 || total <= 1)) {
        state.sent = seen;
        this.sendEvents([{ VisibleRange: { id: v.id, first: range.start, count: range.end - range.start } }]);
      }
      const rows = [];
      for (let i = range.start; i < range.end; i++) {
        const it = v.items[i - first];
        rows.push(it ? this.listRow(v, it, i, t, true).h(rowH).overflow_hidden() : div().w_full().h(rowH));
      }
      return rows;
    }).track_scroll(state.handle).size_full()
      // Rows can't hold handlers of their own here: one for the list, by row number.
      .on_item_click((key, cx) => {
        const i = Number(key), it = v.items[i - first];
        if (!it || it.disabled || it.header) return;
        const list = this.lists.get(v.id);
        if (list) Object.assign(list, { selected: i, local: true });
        this.keyList = v.id;
        const now = Date.now(), again = state.clicked?.i === i && now - state.clicked.at < 400;
        state.clicked = { i, at: now };
        // Clicked "<list>#<row>": a click, not the keys (a folder opens on one click).
        const events = [{ Selected: { id: v.id, index: i } }, { Clicked: { id: `${v.id}#${i}` } }];
        if (again) events.push({ Activated: { id: v.id, index: i } });
        this.sendEvents(events);
        this.focus.focus();
        cx.notify();
      });
    // In a box of the size it's given, which it fills; a scrollbar on the right (where
    // you are; drag it, or click the track).
    let box = div().relative().w_full().child(el)
      // The module's page always shows where it is; other lists' bars show when they move.
      .child(Scrollbar.vertical(`list-${v.id}`).absolute().inset_0().when(Boolean(v.pages), (b) => b.mode("always")));
    if (v.font === "Mono" && settings()?.font_family) box = box.font_family(String(settings().font_family));
    const h = v.layout?.height;
    box = h?.Fixed !== undefined ? box.h(h.Fixed) : ctx.inRow ? box.h_full() : box.flex_1().min_h(0);
    const width = v.layout?.width;
    if (width === "Fill" && ctx.inRow) box = box.flex_1().min_w(0);
    else if (width?.Fixed !== undefined) box = box.w(width.Fixed).flex_shrink_0();
    return box;
  }

  /** One row of a list; `i` counts from the list's first row. In a virtual list
   *  (`virtual`) it holds no handlers: the list has them. */
  listRow(v, it, i, t, virtual = false) {
    const soft = v.style === "Soft";
    {
      if (it.header) {
        return div().px(8).pt(8).pb(2).text_size(11).text_color(t.dim).child(it.label);
      }
      // Drawn from the selection here, which moves at once on a click or key.
      const selected = (this.lists.get(v.id)?.selected ?? v.selected) === i;
      const fg = selected && !soft ? t.accent_foreground : it.disabled ? t.dim : t.foreground;
      // A virtual row with content of its own is flush: its lines and lanes meet.
      const flush = virtual && it.child;
      let row = h_flex().id(`l-${v.id}-${i}`).w_full().gap(8).px(8).py(flush ? 0 : v.dense ? 3 : 6).rounded(flush ? 0 : 6).items_center()
        .text_color(fg);
      if (selected) row = row.bg(soft ? t.selected : t.accent);
      else row = row.hover((s) => s.bg(t.hover));
      if (it.disabled) row = row.opacity(0.5);
      else if (virtual) row = row.cursor_pointer();
      else {
        row = row.cursor_pointer().on_click((e, cx) => {
          const list = this.lists.get(v.id);
          if (list) Object.assign(list, { selected: i, local: true });
          this.keyList = v.id;
          const events = [{ Selected: { id: v.id, index: i } }];
          if (e.click_count >= 2) events.push({ Activated: { id: v.id, index: i } });
          this.sendEvents(events);
          this.focus.focus();
          cx.notify();
        });
      }
      if (it.tooltip) row = row.tooltip(it.tooltip);
      // Its own content, clipped to the row (a virtual row's has no handlers).
      // A flush row's content takes its whole height (a line's fill meets the next).
      if (it.child) return row.child(div().flex_1().min_w(0).overflow_hidden().when(flush, (d) => d.h_full().flex().items_center())
        .child(this.node(it.child, t, { inRow: false, virtual })));
      if (it.tree) row = row.child(div().text_color(t.dim).child(it.tree));
      if (it.dot != null) {
        // On the inverted selection the dot takes the row's text color.
        const c = selected && !soft ? fg : hex(it.dot);
        row = row.child(it.ring
          ? div().w(8).h(8).rounded_full().border_1().border_color(c).flex_shrink_0()
          : div().w(8).h(8).rounded_full().bg(c).flex_shrink_0());
      }
      if (it.icon) row = row.child(div().text_color(it.icon_color != null ? hex(it.icon_color) : t.dim).child(it.icon));
      const roleColor_ = it.role ? roleColor(it.role, t) : null;
      // One line, cut at the room it has (never under the detail); on the inverted
      // selection every run takes the row's text colour, as the detail does.
      const spanColor = (s) => selected && !soft ? fg : s.color != null ? hex(s.color) : fg;
      let label = it.label_spans?.length
        ? h_flex().min_w(0).overflow_hidden().whitespace_nowrap().children(it.label_spans.map((s) => div().flex_shrink_0().text_color(spanColor(s)).child(s.text)))
        : div().min_w(0).truncate().child(it.label);
      if (roleColor_ && !selected) label = label.text_color(roleColor_);
      if (it.bold) label = label.font_semibold();
      let main = v_flex().flex_1().min_w(0).child(label);
      if (it.sub?.length) main = main.child(h_flex().text_size(11.5).children(it.sub.map((s) => div().text_color(s.color != null ? hex(s.color) : t.dim).child(s.text))));
      row = row.child(main);
      if (it.detail) row = row.child(div().text_size(12).text_color(selected && !soft ? fg : it.detail_color != null ? hex(it.detail_color) : t.dim).child(it.detail));
      if (it.key) row = row.child(div().text_size(11.5).text_color(t.accent).child(it.key));
      if (it.trailing && virtual) row = row.child(div().px(6).text_color(t.dim).child(it.trailing.text));
      else if (it.trailing) {
        const id = it.trailing.id;
        row = row.child(div().id(`l-${v.id}-${i}-t`).px(6).rounded(4).text_color(selected && !soft ? fg : t.dim)
          .hover((s) => s.bg(t.hover).text_color(t.foreground)).cursor_pointer()
          .on_click((_e, cx) => { this.sendEvents([{ Clicked: { id } }]); cx.stop_propagation(); })
          .child(it.trailing.text));
      }
      return row;
    }
  }

  // ----- canvases

  /** A canvas's own state here: its size, and its pan and zoom (`view`). */
  canvasState(id) {
    let c = this.canvases.get(id);
    if (!c) this.canvases.set(id, (c = { w: 0, h: 0, view: null, moved: false, reset: 0, down: null, sentView: "" }));
    return c;
  }

  /** The view fitted to the content (centered), or zoom 1 at the origin. */
  fit(c, pz) {
    if (!pz.content || !c.w || !c.h) return { x: 0, y: 0, zoom: 1 };
    const [cw, ch] = pz.content;
    const zoom = Math.max(pz.min_zoom ?? 0.05, Math.min(pz.max_zoom ?? 64, Math.min(c.w / cw, c.h / ch)));
    return { x: -(c.w / zoom - cw) / 2, y: -(c.h / zoom - ch) / 2, zoom };
  }

  /** Tells the module what a canvas with a view shows, when that changed. */
  sendView(id, c) {
    if (!c.view || !c.w) return;
    const v = { id, x: c.view.x, y: c.view.y, width: c.w / c.view.zoom, height: c.h / c.view.zoom, zoom: c.view.zoom };
    const json = JSON.stringify(v);
    if (json === c.sentView) return;
    c.sentView = json;
    this.sendEvents([{ View: v }]);
  }

  /** Zooms the view by `factor` around the point (px, py) in the canvas. */
  zoomAt(c, pz, factor, px, py) {
    const v = c.view;
    const zoom = Math.max(pz.min_zoom ?? 0.05, Math.min(pz.max_zoom ?? 64, v.zoom * factor));
    const wx = v.x + px / v.zoom, wy = v.y + py / v.zoom;
    c.view = { x: wx - px / zoom, y: wy - py / zoom, zoom };
    c.moved = true;
  }

  /** A canvas: shapes placed in a box, in points or fractions of it, optionally panned
   *  and zoomed here (the module draws in world coordinates). */
  canvas(v, t, ctx) {
    const id = v.id;
    const c = this.canvasState(id);
    const pz = v.view ?? null;
    c.node = v;
    // In a virtual list's row, handlers aren't allowed: it's drawn at its own size,
    // and doesn't take the pointer.
    if (ctx.virtual) {
      c.w = v.width;
      c.h = v.height;
    }
    const g = this.canvasGeometry(v, c);
    const { view, zoom, W, H, wx, wy } = g;

    let el = div().id(`cv-${id}`).relative().flex_shrink_0().overflow_hidden();
    el = el.h(v.height || 0);
    if (v.layout?.width === "Fill" || (v.width === 0 && unit)) el = ctx.inRow ? el.flex_1() : el.w_full();
    else el = el.w(v.width);
    el = this.sized(el, v.layout, ctx);
    // In a virtual list's row, handlers aren't allowed: it's drawn at its own size,
    // and doesn't take the pointer.
    if (!ctx.virtual) el = el.on_size_change((reported, cx) => {
      const z = uiZoom(), size = { width: reported.width / z, height: reported.height / z };
      if (c.w === size.width && c.h === size.height) return;
      c.w = size.width;
      c.h = size.height;
      this.prepare(c);
      if (pz) this.sendView(id, c);
      cx.notify();
    });

    if (!ctx.virtual) el = this.canvasInput(el, v, c, pz, id, zoom, wx, wy);
    if (!W || !H) return el;
    return this.canvasDrawn(el, c, g);
  }

  /** A canvas's view (fitted when it should be) and the mappings between its world and
   *  this element's pixels. */
  canvasGeometry(v, c) {
    const unit = v.coords === "Unit";
    const pz = v.view ?? null;
    if (pz) {
      if (!c.view || (pz.reset ?? 0) > c.reset || (!c.moved && c.fitted !== `${c.w}x${c.h}:${pz.content}`)) {
        c.view = this.fit(c, pz);
        c.reset = pz.reset ?? 0;
        c.moved = false;
        c.fitted = `${c.w}x${c.h}:${pz.content}`;
        // The module builds what's in view: tell it (it answers with a new frame).
        if (c.w) this.sendView(v.id, c);
      }
    } else c.view = null;
    const view = c.view ?? { x: 0, y: 0, zoom: 1 };
    const zoom = view.zoom;
    const W = c.w, H = c.h;
    return {
      view, zoom, W, H,
      // World → this element's pixels.
      sx: (n) => ((unit ? n * W : n) - view.x) * zoom,
      sy: (n) => ((unit ? n * H : n) - view.y) * zoom,
      sw: (n) => (unit ? n * W : n) * zoom,
      sh: (n) => (unit ? n * H : n) * zoom,
      // Pixels in this element → the canvas's coordinates (what events carry).
      wx: (px) => { const x = px / zoom + view.x; return unit ? x / (W || 1) : x; },
      wy: (py) => { const y = py / zoom + view.y; return unit ? y / (H || 1) : y; },
    };
  }

  /** Builds what a canvas shows, if what's kept doesn't fit any more, outside a render
   *  (a frame arriving, a resize, a pan or zoom): building thousands of shapes can take
   *  longer than a render may. */
  prepare(c) {
    if (!c.node || !c.w || !c.h) return;
    this.ensureDrawn(c, this.canvasGeometry(c.node, c));
  }

  /** Every canvas in a new frame's tree, prepared (see `prepare`). */
  prepareCanvases(n) {
    if (!n || typeof n !== "object") return;
    const [type, v] = Object.entries(n)[0] ?? [];
    if (type === "Canvas") {
      const c = this.canvases.get(v.id);
      if (c) {
        c.node = v;
        this.prepare(c);
      }
    } else if (type === "Column" || type === "Row") v.children.forEach((k) => this.prepareCanvases(k));
    else if (type === "Popup") this.prepareCanvases(v.child);
  }

  /** A canvas's pointer, pan and wheel handlers. */
  canvasInput(el, v, c, pz, id, zoom, wx, wy) {
    // The pointer: presses, drags and releases (and moves, with hover), or panning.
    const local = (e) => unzoomed(e.local_position) ?? { x: 0, y: 0 };
    const pointer = (kind, e) => {
      const p = local(e);
      this.sendEvents([{ Pointer: { id, x: wx(p.x), y: wy(p.y), kind } }], e.modifiers);
    };
    el = el.on_mouse_down("left", (e, cx) => {
      const p = local(e);
      if (pz?.drag_pans) c.down = { pan: true, x: p.x, y: p.y };
      else { c.down = { pan: false }; pointer("Down", e); }
      this.focus.focus();
      cx.stop_propagation();
    }).on_mouse_down("middle", (e, cx) => {
      if (!pz) return;
      const p = local(e);
      c.down = { pan: true, x: p.x, y: p.y };
      cx.stop_propagation();
    }).on_mouse_move((e, cx) => {
      const p = local(e);
      if (c.down?.pan && c.view) {
        c.view = { ...c.view, x: c.view.x - (p.x - c.down.x) / zoom, y: c.view.y - (p.y - c.down.y) / zoom };
        c.down.x = p.x;
        c.down.y = p.y;
        c.moved = true;
        this.prepare(c);
        cx.notify();
      } else if (c.down) pointer("Drag", e);
      else if (v.sense?.hover) pointer("Move", e);
    });
    const up = (e, cx) => {
      if (!c.down) return;
      const pan = c.down.pan;
      c.down = null;
      if (pan) this.sendView(id, c);
      else pointer("Up", e);
    };
    el = el.on_mouse_up("left", up).on_mouse_up("middle", up);
    if (v.sense?.hover) {
      el = el.on_hover((hovered) => this.sendEvents([{ Pointer: { id, x: 0, y: 0, kind: hovered ? "Enter" : "Leave" } }]));
    }
    if (pz) {
      el = el.on_scroll_wheel((e, cx) => {
        const p = unzoomed(e.local_position) ?? { x: c.w / 2, y: c.h / 2 };
        if (e.modifiers?.control) this.zoomAt(c, pz, Math.exp(e.delta.y * 0.003), p.x, p.y);
        else {
          const d = unzoomed(e.delta);
          const dx = e.modifiers?.shift && !d.x ? d.y : d.x, dy = e.modifiers?.shift && !d.x ? 0 : d.y;
          c.view = { ...c.view, x: c.view.x - dx / zoom, y: c.view.y - dy / zoom };
          c.moved = true;
        }
        this.prepare(c);
        this.sendView(id, c);
        cx.stop_propagation();
        cx.notify();
      }).on_click((e, cx) => {
        if (e.click_count < 2) return;
        c.view = this.fit(c, pz);
        c.moved = false;
        this.prepare(c);
        this.sendView(id, c);
        cx.notify();
      });
    } else if (v.sense?.wheel) {
      el = el.on_scroll_wheel((e, cx) => {
        const p = local(e);
        const at = { x: wx(p.x), y: wy(p.y) };
        if (e.modifiers?.control) this.sendEvents([{ Zoom: { id, factor: Math.exp(e.delta.y * 0.003), ...at } }], e.modifiers);
        else {
          const d = unzoomed(e.delta);
          this.sendEvents([{ Wheel: { id, dx: d.x, dy: d.y, ...at } }], e.modifiers);
        }
        cx.stop_propagation();
      });
    }
    return el;
  }

  /** A canvas's shapes, drawn into `el` (built by `prepare`, or here if nothing kept fits). */
  canvasDrawn(el, c, g) {
    this.ensureDrawn(c, g);
    const { view, zoom } = g;
    const dx = (c.drawn.x - view.x) * zoom, dy = (c.drawn.y - view.y) * zoom;
    const layer = div().absolute().left(dx).top(dy).size_full().children(c.drawn.items.map((it) => (it.path
      ? window.paint_path(it.path, hex(it.color)).absolute().top(0).left(0).size_full()
      : it.make())));
    return el.child(layer);
  }

  /** What a canvas's shapes come to (`c.drawn`), built again unless what's kept fits. */
  ensureDrawn(c, { view, zoom, W, H, sx, sy, sw, sh }) {
    const v = c.node;
    // What the shapes come to is kept until the frame, the zoom or the size changes
    // (built paths are reusable), with a screen's margin around the view: panning only
    // moves it, until the view goes past that margin.
    // Its shapes (and the pictures they show) as last drawn: a frame that changed only
    // other parts doesn't redraw it.
    if (c.shapes !== v.shapes) {
      const sig = JSON.stringify(v.shapes) + JSON.stringify(this.out?.images ?? {});
      if (sig !== c.sig) { c.sig = sig; c.version = (c.version ?? 0) + 1; }
      c.shapes = v.shapes;
    }
    const key = `${c.version}|${zoom}|${uiZoom()}|${W}x${H}`;
    const d = c.drawn;
    if (d?.key !== key || Math.abs(d.x - view.x) * zoom > W || Math.abs(d.y - view.y) * zoom > H) {
      const near = (x, y, w, h) => x + w >= -W && y + h >= -H && x <= 2 * W && y <= 2 * H;
      c.drawn = { key, x: view.x, y: view.y, items: this.shapes(v, { sx, sy, sw, sh, zoom, visible: near }) };
    }
  }

  /** A canvas's shapes in this element's pixels: built paths ({path, color}) and makers of
   *  other elements ({make}), in drawing order. */
  shapes(v, { sx, sy, sw, sh, zoom, visible }) {
    const mono = settings()?.font_family ? String(settings().font_family) : null;
    const images = this.out?.images ?? {};
    const out = [];
    // Plain fills (rectangles, circles, polygons) and lines in a row go into a few native
    // paths, one per color (and width), drawn in the order each first appeared: a canvas
    // of thousands of dots is a handful of elements. Text, pictures and the like end a row.
    let groups = new Map();
    const flush = () => {
      for (const g of groups.values()) out.push({ path: g.builder.build(), color: g.color });
      groups = new Map();
    };
    const group = (key, color, make) => {
      let g = groups.get(key);
      if (!g) groups.set(key, (g = { color, builder: make() }));
      return g;
    };
    // A path's points and stroke width are in the script's pixels: the app zooms them.
    const stroke = (width, color, points) => {
      const g = group(`s/${color}/${width}`, color, () => PathBuilder.stroke(Math.max(0.5, width * zoom)));
      let b = g.builder.move_to(points[0][0], points[0][1]);
      for (const [x, y] of points.slice(1)) b = b.line_to(x, y);
      g.builder = b;
    };
    const fill = (color, points) => {
      const g = group(`f/${color}`, color, () => PathBuilder.fill());
      g.builder = g.builder.add_polygon(points, true);
    };
    for (const s of v.shapes ?? []) {
      const [kind, p] = Object.entries(s)[0];
      const plainRect = kind === "Rect" && !p.stroke && !p.radius;
      if (kind !== "Line" && kind !== "Path" && kind !== "Circle" && !plainRect) flush();
      if (plainRect) {
        const x = sx(p.x), y = sy(p.y), w = sw(p.w), h = sh(p.h);
        if (!visible(x, y, w, h)) continue;
        fill(p.fill, [[x, y], [x + w, y], [x + w, y + h], [x, y + h]]);
      } else if (kind === "Rect") {
        const x = sx(p.x), y = sy(p.y), w = sw(p.w), h = sh(p.h);
        if (!visible(x, y, w, h)) continue;
        const radius = (p.radius ?? 0) * zoom, fillColor = hex(p.fill), border = p.stroke ? hex(p.stroke[1]) : null;
        out.push({ make: () => {
          const r = div().absolute().left(x).top(y).w(w).h(h).bg(fillColor).rounded(radius);
          return border ? r.border_1().border_color(border) : r;
        } });
      } else if (kind === "Circle") {
        const r = p.r * zoom, x = sx(p.x), y = sy(p.y);
        if (r < 0.2 || !visible(x - r, y - r, 2 * r, 2 * r)) continue;
        // A polygon close enough at its size: one call, not an arc per half.
        const unit = r < 2 ? CIRCLE4 : r < 8 ? CIRCLE12 : CIRCLE32;
        fill(p.fill, unit.map(([cx, cy]) => [x + cx * r, y + cy * r]));
      } else if (kind === "Text") {
        const size = p.size * zoom, x = sx(p.x), y = sy(p.y);
        if (size < 2 || !visible(x, y, 0.6 * size * p.text.length, size * 1.4)) continue;
        const color = hex(p.color), text = p.text;
        out.push({ make: () => {
          const tx = div().absolute().left(x).top(y).text_size(size).line_height(1.25).text_color(color).whitespace_nowrap().child(text);
          return mono ? tx.font_family(mono) : tx;
        } });
      } else if (kind === "Line") {
        stroke(p.width, p.color, [[sx(p.x1), sy(p.y1)], [sx(p.x2), sy(p.y2)]]);
      } else if (kind === "Path") {
        if (!p.points?.length) continue;
        const pts = p.points.map(([x, y]) => [sx(x), sy(y)]);
        if (p.fill != null && pts.length > 2) fill(p.fill, pts);
        if (p.stroke && pts.length > 1) stroke(p.stroke[0], p.stroke[1], p.closed ? [...pts, pts[0]] : pts);
      } else if (kind === "Image") {
        const [srcKind, src] = Object.entries(p.src)[0];
        const key = srcKind === "Handle" ? `handle:${src}` : srcKind === "Bytes" ? src.key : src;
        const img = images[key];
        const x = sx(p.x), y = sy(p.y), w = sw(p.w), h = sh(p.h);
        if (!img || !visible(x, y, w, h)) continue;
        // Placed by fit inside its box (the box clips: Cover).
        let iw = w, ih = h;
        if (p.fit !== "Fill" && img.width && img.height) {
          const scale = (p.fit === "Cover" ? Math.max : Math.min)(w / img.width, h / img.height);
          iw = img.width * scale;
          ih = img.height * scale;
        }
        const rounding = (p.rounding ?? 0) * zoom, opacity = p.opacity;
        out.push({ make: () => {
          const box = div().absolute().left(x).top(y).w(w).h(h).overflow_hidden().rounded(rounding)
            .child(image(`${IMAGES}/${img.file}`).absolute().left((w - iw) / 2).top((h - ih) / 2).w(iw).h(ih));
          return opacity != null && opacity < 1 ? box.opacity(opacity) : box;
        } });
      }
    }
    flush();
    return out;
  }
}
