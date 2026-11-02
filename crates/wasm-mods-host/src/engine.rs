//! Running a module: wasmtime (Cranelift) with no imports, one memory and one table
//! with caps, and the same proposals off as the SDK builds with. A call is held by
//! time: an epoch ticks every [`TICK`] while calls run, and a call past its
//! [`Limits::time`] traps.
//!
//! Compiling takes ~100 ms a module, so compiled modules are kept on disk
//! (`~/.cache/wasm-mods/wasmtime/<sha256 of the .wasm>-<engine key>.cwasm`): later
//! loads map them in a fraction of a millisecond. The key covers wasmtime's version and
//! this engine's settings; a file wasmtime doesn't accept is compiled again.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use wasmtime::{
    Config, Engine, Linker, Memory, Module, OptLevel, Store, StoreLimits, StoreLimitsBuilder, Strategy, Trap, TypedFunc,
};

/// The pinned wasmtime (Cargo.toml), part of the cache key.
const VERSION: &str = "50.0.0";
/// How often the epoch ticks while calls run.
const TICK: Duration = Duration::from_millis(5);
/// The most compiled modules kept on disk (oldest go first).
const CACHE_FILES: usize = 64;
/// The most a module may answer in one call.
const MAX_ANSWER: usize = 32 << 20;

#[derive(Clone, Debug, PartialEq)]
pub struct Error(pub String);

impl Error {
    pub fn new(s: impl Into<String>) -> Self {
        Error(s.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<wasmtime::Error> for Error {
    fn from(e: wasmtime::Error) -> Self {
        match e.downcast_ref::<Trap>() {
            Some(Trap::Interrupt) => Error::new("out of time: the call ran past its time limit"),
            _ => Error(format!("{e:#}")),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// How tightly a module is held: the time a call may take and its memory.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub time: Duration,
    pub memory: usize,
}

/// Every module: about half a second per call, 256 MB.
pub const LIMITS: Limits = Limits { time: Duration::from_millis(500), memory: 256 << 20 };

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut c = Config::new();
        c.strategy(Strategy::Cranelift);
        c.cranelift_opt_level(OptLevel::Speed);
        c.epoch_interruption(true);
        // What mods are built with: one memory, standard pages, no wide arithmetic,
        // SIMD or GC (threads and components aren't compiled in).
        c.wasm_multi_memory(false);
        c.wasm_custom_page_sizes(false);
        c.wasm_wide_arithmetic(false);
        c.wasm_relaxed_simd(false);
        c.wasm_simd(false);
        c.wasm_gc(false);
        c.max_wasm_stack(1 << 20);
        Engine::new(&c).expect("wasmtime's settings")
    })
}

/// Milliseconds since the ticker's start at the last call armed ([`arm`]).
static ARMED: AtomicU64 = AtomicU64::new(0);
/// Calls running now (armed and not yet returned, however they ended).
static RUNNING: AtomicUsize = AtomicUsize::new(0);

/// A call running: counted from [`arm`] until it's dropped (returned, failed or trapped).
#[must_use]
struct Running;

impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Whether the ticker may sleep: no call running, and none armed for two seconds. Never
/// while one runs, however slow the ticks come (a busy machine's 5 ms sleeps can take
/// far longer, and a call stopped by no tick would run forever).
fn may_park(running: usize, now_ms: u64, armed_ms: u64) -> bool {
    running == 0 && now_ms.saturating_sub(armed_ms) > 2000
}

fn start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// The thread that moves the epoch on: every [`TICK`] while calls run, asleep two
/// seconds after the last one ended. An [`arm`] wakes it; one that comes between its
/// check and its sleep isn't lost (`unpark` before `park` makes `park` return).
fn ticker() -> &'static std::thread::Thread {
    static TICKER: OnceLock<std::thread::Thread> = OnceLock::new();
    TICKER.get_or_init(|| {
        std::thread::Builder::new()
            .name("wasm-epoch".into())
            .spawn(|| {
                loop {
                    std::thread::sleep(TICK);
                    engine().increment_epoch();
                    let now = start().elapsed().as_millis() as u64;
                    if may_park(RUNNING.load(Ordering::SeqCst), now, ARMED.load(Ordering::Relaxed)) {
                        std::thread::park();
                    }
                }
            })
            .expect("starting the wasm epoch thread")
            .thread()
            .clone()
    })
}

/// Before a call: its deadline, and the ticker awake until the returned [`Running`] is
/// dropped.
fn arm(store: &mut Store<StoreLimits>, time: Duration) -> Running {
    RUNNING.fetch_add(1, Ordering::SeqCst);
    ARMED.store(start().elapsed().as_millis() as u64, Ordering::Relaxed);
    ticker().unpark();
    store.set_epoch_deadline((time.as_millis() / TICK.as_millis()).max(1) as u64 + 1);
    Running
}

fn cache_dir() -> Option<PathBuf> {
    // Tests' throwaway modules stay off the user's disk.
    if cfg!(test) {
        return None;
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("wasm-mods").join("wasmtime"))
}

/// This engine's part of the key: wasmtime's version and its settings.
fn engine_key() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        VERSION.hash(&mut h);
        engine().precompile_compatibility_hash().hash(&mut h);
        format!("{:016x}", h.finish())
    })
}

/// Compiles a module into the disk cache (if it isn't there yet), so it loads at once
/// when it's first shown.
pub fn precompile(bytes: &[u8], hash: &str) -> Result<()> {
    compile(bytes, hash).map(drop)
}

/// The module compiled, from the disk cache when it's there.
fn compile(bytes: &[u8], hash: &str) -> Result<Module> {
    let path = cache_dir().map(|d| d.join(format!("{hash}-{}.cwasm", engine_key())));
    if let Some(path) = &path
        && path.exists()
    {
        // Safety: the file is one this code wrote (`Module::serialize`) into the user's
        // own cache; wasmtime checks its header (version, settings) and refuses others.
        match unsafe { Module::deserialize_file(engine(), path) } {
            Ok(module) => return Ok(module),
            Err(e) => eprintln!("wasm-mods: cached {} unusable, compiling again: {e:#}", path.display()),
        }
    }
    let module = Module::new(engine(), bytes)?;
    if let Some(path) = &path {
        store_compiled(&module, path);
    }
    Ok(module)
}

fn store_compiled(module: &Module, path: &std::path::Path) {
    let Some(dir) = path.parent() else { return };
    let written = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let data = module.serialize().map_err(|e| std::io::Error::other(format!("{e:#}")))?;
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = written {
        eprintln!("wasm-mods: caching compiled module {}: {e}", path.display());
        return;
    }
    // Old builds go, the newest kept.
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<_> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "cwasm"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() > CACHE_FILES {
        files.sort();
        for (_, old) in &files[..files.len() - CACHE_FILES] {
            let _ = std::fs::remove_file(old);
        }
    }
}

/// A module instantiated and ready to call.
pub struct Instance {
    store: Store<StoreLimits>,
    time: Duration,
    memory: Memory,
    alloc: TypedFunc<u32, u32>,
    free: TypedFunc<(u32, u32), ()>,
    update: TypedFunc<(u32, u32), u64>,
    update_bytes: Option<TypedFunc<(u32, u32, u32, u32), u64>>,
    save: TypedFunc<(), u64>,
    restore: TypedFunc<(u32, u32), ()>,
}

impl Instance {
    /// Compiles (or maps from the cache) and instantiates build `bytes` (SHA-256 `hash`).
    pub fn load(bytes: &[u8], hash: &str) -> Result<Instance> {
        let module = compile(bytes, hash)?;
        // No imports: a module can only compute and answer.
        if let Some(import) = module.imports().next() {
            return Err(Error::new(format!(
                "imports {}::{}: WASM mods can't import anything (build for wasm32-unknown-unknown, no WASI)",
                import.module(),
                import.name()
            )));
        }
        if module.get_export("wm_update").is_none() {
            return Err(Error::new("not a WASM mod: it doesn't export wm_update"));
        }
        let limits = LIMITS;
        let store_limits = StoreLimitsBuilder::new()
            .memory_size(limits.memory)
            .memories(1)
            .tables(1)
            .table_elements(10_000)
            .instances(1)
            .build();
        let mut store = Store::new(engine(), store_limits);
        store.limiter(|l| l);
        // The start function and the version call share one call's time.
        let _running = arm(&mut store, limits.time);
        let instance = Linker::new(engine()).instantiate(&mut store, &module)?;
        let version: TypedFunc<(), u32> = instance.get_typed_func(&mut store, "wm_protocol_version")?;
        let protocol = version.call(&mut store, ())?;
        if !wasm_mods_protocol::supported(protocol) {
            return Err(Error::new(format!(
                "built for protocol {protocol}, this host runs {}–{}: install a host that runs it, or rebuild the module",
                wasm_mods_protocol::PROTOCOL_MIN,
                wasm_mods_protocol::PROTOCOL_VERSION
            )));
        }
        Ok(Instance {
            memory: instance.get_memory(&mut store, "memory").ok_or_else(|| Error::new("no memory export"))?,
            alloc: instance.get_typed_func(&mut store, "wm_alloc")?,
            free: instance.get_typed_func(&mut store, "wm_free")?,
            update: instance.get_typed_func(&mut store, "wm_update")?,
            update_bytes: instance.get_typed_func(&mut store, "wm_update_bytes").ok(),
            save: instance.get_typed_func(&mut store, "wm_save")?,
            restore: instance.get_typed_func(&mut store, "wm_restore")?,
            time: limits.time,
            store,
        })
    }

    /// One call: `json` (and `side`, raw bytes beside it, for a module exporting
    /// `wm_update_bytes`) in; the answer's JSON and its raw bytes out.
    pub fn call(&mut self, json: &[u8], side: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        let _running = arm(&mut self.store, self.time);
        let ptr = self.put(json)?;
        let len = json.len() as u32;
        match self.update_bytes.clone() {
            Some(f) => {
                let bytes_ptr = self.put(side)?;
                let packed = f.call(&mut self.store, (ptr, len, bytes_ptr, side.len() as u32))?;
                let answer = self.read_packed(packed)?;
                let (json, bytes) = split_answer(&answer)?;
                Ok((json.to_vec(), bytes.to_vec()))
            }
            None => {
                let packed = self.update.call(&mut self.store, (ptr, len))?;
                Ok((self.read_packed(packed)?, Vec::new()))
            }
        }
    }

    /// The module's state as JSON (kept across a reload of a new build).
    pub fn save(&mut self) -> Result<Vec<u8>> {
        let _running = arm(&mut self.store, self.time);
        let packed = self.save.call(&mut self.store, ())?;
        self.read_packed(packed)
    }

    pub fn restore(&mut self, state: &[u8]) -> Result<()> {
        let _running = arm(&mut self.store, self.time);
        let ptr = self.put(state)?;
        Ok(self.restore.call(&mut self.store, (ptr, state.len() as u32))?)
    }

    /// Copies `bytes` into the module's memory (it takes ownership).
    fn put(&mut self, bytes: &[u8]) -> Result<u32> {
        let ptr = self.alloc.call(&mut self.store, bytes.len() as u32)?;
        // Bounds-checked: a pointer outside its memory fails the call.
        self.memory.write(&mut self.store, ptr as usize, bytes).map_err(|e| Error(e.to_string()))?;
        Ok(ptr)
    }

    fn read_packed(&mut self, packed: u64) -> Result<Vec<u8>> {
        let (ptr, len) = ((packed >> 32) as u32, (packed & 0xffff_ffff) as u32);
        // Checked before anything is allocated for it.
        if !answer_fits(ptr, len, self.memory.data_size(&self.store)) {
            return Err(Error::new(format!("answer of {len} bytes at {ptr} is out of bounds or too big")));
        }
        let mut out = vec![0; len as usize];
        self.memory.read(&self.store, ptr as usize, &mut out).map_err(|e| Error(e.to_string()))?;
        self.free.call(&mut self.store, (ptr, len))?;
        Ok(out)
    }
}

/// Whether `(ptr, len)` lies inside a memory of `size` bytes and within [`MAX_ANSWER`].
fn answer_fits(ptr: u32, len: u32, size: usize) -> bool {
    len as usize <= MAX_ANSWER && (ptr as u64 + len as u64) <= size as u64
}

/// `[json length: u32 LE][json][bytes]`, the answer of `wm_update_bytes`, split.
fn split_answer(answer: &[u8]) -> Result<(&[u8], &[u8])> {
    let bad = || Error::new("malformed answer");
    let n = u32::from_le_bytes(answer.get(..4).ok_or_else(bad)?.try_into().map_err(|_| bad())?) as usize;
    let json = answer.get(4..4usize.checked_add(n).ok_or_else(bad)?).ok_or_else(bad)?;
    Ok((json, &answer[4 + n..]))
}

/// The `wasm_mods.manifest` custom section of a wasm binary, as text.
pub fn manifest_text(wasm: &[u8]) -> Option<String> {
    custom_section(wasm, "wasm_mods.manifest").map(|b| String::from_utf8_lossy(b).into_owned())
}

/// Whether a binary is a WASM mod (it exports `wm_update`), read straight from the binary
/// format (no compiling).
pub fn is_module(wasm: &[u8]) -> bool {
    export_names(wasm).is_some_and(|names| names.iter().any(|n| n == "wm_update"))
}

fn export_names(wasm: &[u8]) -> Option<Vec<String>> {
    if wasm.get(..8)? != b"\0asm\x01\0\0\0" {
        return None;
    }
    let mut at = 8;
    while at < wasm.len() {
        let id = wasm[at];
        at += 1;
        let (size, n) = leb128(wasm.get(at..)?)?;
        at += n;
        let body = wasm.get(at..at.checked_add(size as usize)?)?;
        at += size as usize;
        if id == 7 {
            let (count, mut p) = leb128(body)?;
            let mut names = Vec::new();
            for _ in 0..count {
                let (len, n) = leb128(body.get(p..)?)?;
                p += n;
                names.push(String::from_utf8_lossy(body.get(p..p.checked_add(len as usize)?)?).into_owned());
                p += len as usize + 1;
                let (_, n) = leb128(body.get(p..)?)?;
                p += n;
            }
            return Some(names);
        }
    }
    Some(Vec::new())
}

/// The first custom section named `name`, read straight from the binary format.
fn custom_section<'a>(wasm: &'a [u8], name: &str) -> Option<&'a [u8]> {
    if wasm.get(..8)? != b"\0asm\x01\0\0\0" {
        return None;
    }
    let mut at = 8;
    while at < wasm.len() {
        let id = wasm[at];
        at += 1;
        let (size, n) = leb128(wasm.get(at..)?)?;
        at += n;
        let body = wasm.get(at..at.checked_add(size as usize)?)?;
        at += size as usize;
        if id == 0 {
            let (len, n) = leb128(body)?;
            let section = body.get(n..n.checked_add(len as usize)?)?;
            if section == name.as_bytes() {
                return Some(&body[n + len as usize..]);
            }
        }
    }
    None
}

/// An unsigned LEB128 number and how many bytes it took.
fn leb128(bytes: &[u8]) -> Option<(u32, usize)> {
    let mut value: u32 = 0;
    for (i, b) in bytes.iter().take(5).enumerate() {
        value |= ((b & 0x7f) as u32) << (7 * i);
        if b & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(entry: &str, body: &str) -> Vec<u8> {
        wat::parse_str(format!(
            r#"(module
                (memory (export "memory") 1)
                (func (export "wm_protocol_version") (result i32) (i32.const {v}))
                (func (export "wm_alloc") (param i32) (result i32) (i32.const 1024))
                (func (export "wm_free") (param i32 i32))
                (func (export "wm_save") (result i64) (i64.const 0))
                (func (export "wm_restore") (param i32 i32))
                (func (export "{entry}") (param i32 i32) (result i64) {body}))"#,
            v = wasm_mods_protocol::PROTOCOL_VERSION
        ))
        .unwrap()
    }

    #[test]
    fn the_ticker_never_sleeps_while_a_call_runs() {
        // Slow ticks: a call armed 10 s ago and still running keeps the ticker going.
        assert!(!may_park(1, 12_000, 2_000));
        assert!(!may_park(3, 60_000, 0));
        // Idle for two seconds: it sleeps; not before.
        assert!(may_park(0, 4_001, 2_000));
        assert!(!may_park(0, 3_000, 2_000));
    }

    #[test]
    fn hostile_modules_are_stopped() {
        // A loop runs out of time, an answer outside memory is refused, imports never load.
        let spin = module("wm_update", "(loop (br 0)) (unreachable)");
        let mut i = Instance::load(&spin, "spin").unwrap();
        assert!(i.call(b"{}", b"").unwrap_err().0.contains("out of time"));
        let wild = module("wm_update", "(i64.const 0xffff_0000_0000_0010)");
        let mut i = Instance::load(&wild, "wild").unwrap();
        assert!(i.call(b"{}", b"").unwrap_err().0.contains("out of bounds"));
        let imports = wat::parse_str(r#"(module (import "env" "x" (func)))"#).unwrap();
        assert!(Instance::load(&imports, "imp").err().unwrap().0.contains("can't import"));
        let other = module("wm_other", "(i64.const 0)");
        assert!(Instance::load(&other, "o").err().unwrap().0.contains("not a WASM mod"));
        assert!(is_module(&spin) && !is_module(&other) && !is_module(&imports));
    }
}

/// Replays a module's recorded inputs through the engine, with its frame limits:
/// `REPLAY_WASM=<module.wasm> REPLAY_INPUTS=<dir of input-*.json> cargo test --release
/// -p wasm-mods-host replay -- --ignored --nocapture` (the diff module's bench writes
/// them: DIFF_BENCH_DUMP). Prints each call's time, and the error if one stops it.
#[cfg(test)]
mod replay {
    #[test]
    #[ignore]
    fn replay() {
        let (Ok(wasm), Ok(dir)) = (std::env::var("REPLAY_WASM"), std::env::var("REPLAY_INPUTS")) else { return };
        let bytes = std::fs::read(&wasm).unwrap();
        let mut instance = super::Instance::load(&bytes, "replay").unwrap();
        let mut inputs: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok().map(|e| e.path())).collect();
        inputs.sort();
        let mut worst = 0.0f64;
        for path in inputs {
            let json = std::fs::read(&path).unwrap();
            let t = std::time::Instant::now();
            let r = instance.call(&json, &[]);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            worst = worst.max(ms);
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if let (Ok((out, _)), Ok(dir)) = (&r, std::env::var("REPLAY_OUT")) {
                let _ = std::fs::write(format!("{dir}/{name}"), out);
            }
            match r {
                Ok(_) if ms >= 20.0 => println!("REPLAY {name} {ms:.1} ms"),
                Ok(_) => {}
                Err(e) => {
                    println!("REPLAY {name} {ms:.1} ms STOPPED: {e}");
                    break;
                }
            }
        }
        println!("REPLAY worst {worst:.1} ms");
    }
}
