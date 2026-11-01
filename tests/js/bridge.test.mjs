// The parts of the bridge that decide what runs: which host, whether a file is the one
// pinned, what host.lock.json says.  node --import ./tests/js/register.mjs --test tests/js/bridge.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  compareVersions, fits, folderOf, installedName, pickHost, parseLock, parseSha256sum, sameHash, parseRunFile, shouldTakeOver,
} from "../../wasm-mods/mods/wasm-mods/wasm-bridge.js";

const H = (version, min, max) => ({ version, protocol: [min, max], path: `/h/${version}` });
const SUM = "a".repeat(64);

test("picks the newest installed host that runs the module's protocol", () => {
  const hosts = [H("0.2.0", 4, 4), H("0.10.0", 4, 5), H("0.3.1", 5, 6), H("bad", 1, 9)];
  assert.equal(pickHost(hosts, 4).version, "0.10.0"); // 0.10 is newer than 0.2 (not by text)
  assert.equal(pickHost(hosts, 6).version, "0.3.1");
  assert.equal(pickHost(hosts, 3), null);
  assert.equal(pickHost([], 4), null);
  assert.ok(compareVersions("1.0.0", "0.99.99") > 0 && compareVersions("0.2.0-rc1", "0.2.0") === 0);
  assert.ok(fits(H("0.2.0", 4, 5), 5) && !fits(H("0.2.0", 4, 5), 6));
});

test("a newer fitting host takes over; the running one stays if it's the newest that fits", () => {
  const hosts = [H("0.2.0", 4, 4), H("0.3.0", 4, 5)];
  const run = (version) => ({ port: 1, token: "t", version });
  assert.equal(shouldTakeOver(run("0.3.0"), pickHost(hosts, 4), hosts, 4), false);
  assert.equal(shouldTakeOver(run("0.2.0"), pickHost(hosts, 4), hosts, 4), true);
  // A host from before versions (0.0.0, protocol 4) gives way, also to the first release.
  assert.equal(shouldTakeOver(parseRunFile(`9 ${"b".repeat(48)}\n`), pickHost(hosts, 4), hosts, 4), true);
  // Running a newer one than this copy would pick, which runs its protocol: kept.
  assert.equal(shouldTakeOver(run("0.9.0"), pickHost(hosts, 4), [...hosts, H("0.9.0", 4, 9)], 4), false);
});

test("the run file: port, token and the host's version", () => {
  assert.deepEqual(parseRunFile(`4242 ${"c".repeat(48)} 0.2.0\n`), { port: 4242, token: "c".repeat(48), version: "0.2.0" });
  assert.equal(parseRunFile(`4242 ${"c".repeat(48)}\n`).version, "0.0.0");
  const first = [H("0.1.0", 4, 4)];
  assert.equal(shouldTakeOver(parseRunFile(`9 ${"b".repeat(48)}\n`), pickHost(first, 4), first, 4), true);
  assert.equal(parseRunFile(`4242 ${"c".repeat(47)}\n`), null);
  assert.equal(parseRunFile(""), null);
});

test("hash check: sha256sum's output, compared whole", () => {
  assert.equal(parseSha256sum(`${SUM.toUpperCase()}  /x/wasm-mods-host\n`), SUM);
  assert.equal(parseSha256sum("sha256sum: x: No such file or directory"), null);
  assert.ok(sameHash(SUM, SUM.toUpperCase()));
  assert.ok(!sameHash(SUM, "a".repeat(63) + "b"));
  assert.ok(!sameHash("", ""), "nothing is never a match");
  assert.ok(!sameHash(null, null));
});

const lock = (host, module = null) => JSON.stringify({ host: { version: "0.2.0", protocol: [4, 4], files: {
  "linux-x86_64": { url: "https://example.org/v0.2.0/wasm-mods-host-x86_64", sha256: SUM, size: 123 } }, source: null, ...host }, module });

test("host.lock.json: read and checked", () => {
  const l = parseLock(lock({ source: { git: "https://example.org/wasm-mods", tag: "v0.2.0" } }, { name: "todo", protocol: 4, sha256: SUM }));
  assert.equal(l.host.version, "0.2.0");
  assert.deepEqual(l.host.protocol, [4, 4]);
  assert.equal(l.host.files["linux-x86_64"].size, 123);
  assert.equal(l.host.source.tag, "v0.2.0");
  assert.deepEqual(l.module, { name: "todo", protocol: 4, sha256: SUM });
  // Other platforms are skipped, not refused.
  assert.deepEqual(Object.keys(parseLock(lock({ files: { "macos-arm64": { url: "x", sha256: "y" } } })).host.files), []);
  const bad = [
    "not json",
    lock({ version: "latest" }),
    lock({ protocol: [5, 4] }),
    lock({ protocol: [4] }),
    lock({ files: { "linux-x86_64": { url: "http://example.org/h", sha256: SUM } } }),
    lock({ files: { "linux-x86_64": { url: "https://example.org/h", sha256: "abc" } } }),
    lock({ source: { git: "https://example.org/r" } }),
    lock({}, { name: "Todo!", protocol: 4 }),
  ];
  for (const text of bad) assert.throws(() => parseLock(text), /host\.lock\.json/, text);
});

test("every WASM mod folder has the same wasm-bridge.js", () => {
  execFileSync(new URL("../../scripts/sync-bridge.sh", import.meta.url).pathname, ["--check"]);
});

test("the core mod loads: every name it imports from the bridge is exported", async () => {
  await import("../../wasm-mods/mods/wasm-mods/main.js");
});

test("the mod folder, from where the bridge was loaded", () => {
  assert.equal(folderOf("Error\n    at <anonymous> (/home/u/.config/herdr-app/mods/wasm-todo-2/wasm-bridge.js?v=3:975:20)"), "wasm-todo-2");
  assert.equal(folderOf("Error\n    at x (/repo/mods/wasm-bridge.js:1:1)"), null);
});

test("a mod folder's module is installed under the folder's name", () => {
  assert.equal(installedName("wasm-diff"), "wasm-diff");
  assert.equal(installedName("Someone's Diff 2"), "someone-s-diff-2");
  assert.notEqual(installedName("wasm-diff"), installedName("wasm-diff-2"));
});
