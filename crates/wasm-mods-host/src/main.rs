//! wasm-mods-host: runs WASM mods for herdr-app's `wasm-mods` mod and enforces their
//! permissions.
//!
//! The `wasm-mods` JavaScript mod starts it in a hidden herdr-core terminal
//! (`hc run --bg`): `wasm-mods-host --token-file F --port-file P`. `F` holds the token
//! every connection must open with; it lies in a folder only its owner can enter (the
//! host checks, then deletes the file), so the token never shows in the process list or
//! on screen. The host listens on 127.0.0.1, on a port the system picks, writes it to `P`
//! and prints it. With `--host-file H` it also writes `<port> <token> <version>` to `H`
//! (the file every copy of the mod finds the host by), at once, so the copies find it
//! even if the one that started it is gone; it removes `H` when it exits idle, if `H`
//! still names it. Each connection runs one module ([`session`]); the host exits once no
//! connection has been open for ten minutes.
//!
//! `wasm-mods-host --version [--json]` prints its version and the protocols it runs
//! (`{"version": "0.1.0", "protocol": [4, 4]}`). `wasm-mods-host --list` prints the
//! installed modules as JSON (`[{name, description}]`, for pickers) and exits.
//! `--permissions`, `--allow`, `--deny`, `--forget`, `--enable`, `--disable` and
//! `--remove` show and change what modules may do ([`cli`]).
//!
//! Modules are `DIR/<name>.wasm`, `--modules DIR` (default
//! `~/.local/share/wasm-mods/modules`). `WASM_MODS_TOKEN` in the environment replaces
//! the token file (tests, debugging).

use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

mod canvas;
mod cli;
mod engine;
mod extensions;
mod fs;
mod images;
mod permissions;
mod session;
mod tools;
mod wire;

/// How long the host stays up with no connection: after the last one closed (kept a
/// while, so the next module shown needn't start one), and after it started.
const IDLE: Duration = Duration::from_secs(600);
const FIRST_WITHIN: Duration = Duration::from_secs(30);

/// The value of `--name VALUE`.
fn arg(name: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == name {
            return args.next();
        }
    }
    None
}

fn modules_dir() -> PathBuf {
    if let Some(dir) = arg("--modules") {
        return PathBuf::from(dir);
    }
    std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("wasm-mods/modules")
}

/// The token: from `WASM_MODS_TOKEN`, else the `--token-file`, which must be ours, in a
/// folder that is ours and closed to everyone else; it is deleted once read.
fn read_token() -> Result<String, String> {
    if let Some(t) = std::env::var("WASM_MODS_TOKEN").ok().filter(|t| !t.is_empty()) {
        return Ok(t);
    }
    let path = PathBuf::from(arg("--token-file").ok_or("no --token-file")?);
    let token = token_from(&path);
    let _ = std::fs::remove_file(&path);
    token
}

fn token_from(path: &Path) -> Result<String, String> {
    use std::os::unix::fs::MetadataExt;
    let uid = unsafe { libc::getuid() };
    let dir = path.parent().ok_or("the token file has no folder")?;
    let d = std::fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !d.is_dir() || d.uid() != uid || d.mode() & 0o077 != 0 {
        return Err(format!("{} must be a folder of yours that nobody else can open (mode 700)", dir.display()));
    }
    let f = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !f.is_file() || f.uid() != uid {
        return Err(format!("{} isn't a file of yours", path.display()));
    }
    let token = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?.trim().to_owned();
    if token.len() < 16 {
        return Err("the token is shorter than 16 characters".into());
    }
    Ok(token)
}

/// Writes `text` to `path` whole (a reader never sees half of it).
fn write_whole(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// The installed modules, by name, with their description.
fn list() {
    let mut out = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(modules_dir()).into_iter().flatten().flatten().map(|e| e.path()).collect();
    files.sort();
    for path in files {
        let Some(name) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".wasm")) else { continue };
        if !wire::module_name_ok(name) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        // Turned off (the permissions screen, the host CLI): not offered.
        if !engine::is_module(&bytes) || extensions::is_off(name) {
            continue;
        }
        let description = permissions::read_manifest(&bytes).map(|m| permissions::one_line(&m.description, 120)).unwrap_or_default();
        out.push(serde_json::json!({ "name": name, "description": description }));
    }
    println!("{}", serde_json::Value::Array(out));
}

/// Compiles every installed module not compiled yet (new builds), into the cache.
fn precompile() {
    let Ok(dir) = std::fs::read_dir(modules_dir()) else { return };
    for path in dir.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "wasm")) {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if engine::is_module(&bytes)
            && let Err(e) = engine::precompile(&bytes, &permissions::hash(&bytes))
        {
            eprintln!("wasm-mods: compiling {}: {e:#}", path.display());
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(cmd) = cli::parse(&args) {
        match cmd.and_then(|c| cli::run(&modules_dir(), &c)) {
            Ok(out) => println!("{out}"),
            Err(e) => {
                eprintln!("wasm-mods-host: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if std::env::args().any(|a| a == "--version") {
        let (version, range) = (env!("CARGO_PKG_VERSION"), [wasm_mods_protocol::PROTOCOL_MIN, wasm_mods_protocol::PROTOCOL_VERSION]);
        if std::env::args().any(|a| a == "--json") {
            println!("{}", serde_json::json!({ "version": version, "protocol": range }));
        } else {
            println!("wasm-mods-host {version} (protocol {}–{})", range[0], range[1]);
        }
        return;
    }
    if std::env::args().any(|a| a == "--list") {
        return list();
    }
    if std::env::args().any(|a| a == "--precompile") {
        return precompile();
    }
    let token = match read_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("wasm-mods-host: {e}");
            std::process::exit(2);
        }
    };
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            eprintln!("wasm-mods-host: can't listen on 127.0.0.1: {e}");
            std::process::exit(1);
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    // New builds compiled while nothing asks for them yet.
    std::thread::spawn(precompile);
    println!("wasm-mods-host {} listening on 127.0.0.1:{port}", env!("CARGO_PKG_VERSION"));
    let _ = std::io::stdout().flush();
    if let Some(p) = arg("--port-file")
        && let Err(e) = write_whole(Path::new(&p), &format!("{port}\n"))
    {
        eprintln!("wasm-mods-host: writing {p}: {e}");
    }
    let host_file = arg("--host-file").map(PathBuf::from);
    // The version, so a copy that has a newer host can tell this one is older.
    let mine = format!("{port} {token} {}\n", env!("CARGO_PKG_VERSION"));
    if let Some(h) = &host_file
        && let Err(e) = write_whole(h, &mine)
    {
        eprintln!("wasm-mods-host: writing {}: {e}", h.display());
    }

    let config = Arc::new(session::Config { token: token.clone(), modules: modules_dir(), started: Instant::now() });
    let open = Arc::new(AtomicUsize::new(0));
    let watch = open.clone();
    std::thread::spawn(move || {
        let start = Instant::now();
        let mut idle_since: Option<Instant> = None;
        let mut seen = false;
        loop {
            std::thread::sleep(Duration::from_millis(500));
            if watch.load(Ordering::SeqCst) > 0 {
                seen = true;
                idle_since = None;
                continue;
            }
            let since = *idle_since.get_or_insert_with(Instant::now);
            if (seen && since.elapsed() > IDLE) || (!seen && start.elapsed() > FIRST_WITHIN) {
                // Copies that look for a host then start one, not knock at a closed port.
                if let Some(h) = &host_file
                    && std::fs::read_to_string(h).is_ok_and(|t| t == mine)
                {
                    let _ = std::fs::remove_file(h);
                }
                println!("no connections: exiting");
                std::process::exit(0);
            }
        }
    });
    for stream in listener.incoming().flatten() {
        let (config, open) = (config.clone(), open.clone());
        open.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            session::serve(stream, &config);
            open.fetch_sub(1, Ordering::SeqCst);
        });
    }
}
