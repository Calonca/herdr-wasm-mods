//! One connection: one module shown in herdr-app. Loads it, asks for its permissions,
//! calls it on every event, carries out its effects and
//! sends back what to draw. A new build of the module's file is loaded in place, keeping
//! its state.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use wasm_mods_protocol::{
    Effect, Event, FsReply, FsResult, FsWatchEvent, HostData, Input, Mods, Node, Offer, OpenTarget, Output, Theme, ToolResult,
    WindowContext,
};

use crate::engine::Instance;
use crate::fs::Files;
use crate::fs::portal::{self, Picked};
use std::sync::mpsc::Receiver;
use crate::images::{Images, Shown};
use crate::permissions::{self, Capability, Gate, ModuleId};
use crate::wire::{self, In, Out};

/// How long the first line (the hello) may take.
const HELLO_WITHIN: Duration = Duration::from_secs(5);
/// How often a module's file is checked for a new build.
const TICK: Duration = Duration::from_secs(1);
/// How often watched files are looked at.
const POLL: Duration = Duration::from_millis(250);
/// The most calls in a row one event may cause (a call answering file requests causes
/// the next).
const CHAIN_MAX: usize = 8;

/// The longest line accepted from the JavaScript mod.
const LINE_MAX: usize = 4 << 20;

pub struct Config {
    pub token: String,
    pub modules: PathBuf,
    pub started: Instant,
}

/// Serves one connection until it closes. Errors end it quietly: the mod reconnects.
pub fn serve(stream: TcpStream, config: &Config) {
    let _ = stream.set_nodelay(true);
    let Ok(read_half) = stream.try_clone() else { return };
    let mut reader = BufReader::new(read_half);
    let _ = stream.set_read_timeout(Some(HELLO_WITHIN));
    let Some(first) = read_line(&mut reader) else { return };
    let Ok(In::Hello { token, module, arg, images, open, native_canvas }) = serde_json::from_slice::<In>(&first) else { return };
    if !same(token.as_bytes(), config.token.as_bytes()) {
        eprintln!("wasm-mods: a connection with the wrong token; closed");
        return;
    }
    let mut s = Session::new(stream, config, arg, images.as_deref());
    s.opened(open.as_deref());
    s.native_canvas = native_canvas;
    if let Err(e) = s.load(&module) {
        s.send(&Out::Error { message: &e });
        return;
    }
    let _ = s.stream.set_read_timeout(Some(POLL));
    loop {
        match reader.fill_buf() {
            Ok([]) => return,
            Ok(_) => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                s.tick();
                continue;
            }
            Err(_) => return,
        }
        let Some(line) = read_line(&mut reader) else { return };
        s.got = wire::now_ms();
        match serde_json::from_slice::<In>(&line) {
            Ok(msg) => s.handle(msg),
            Err(e) => eprintln!("wasm-mods: a line it doesn't understand: {e}"),
        }
        if s.closed {
            return;
        }
    }
}

/// One line, without its newline; `None` at the end or past [`LINE_MAX`].
fn read_line(reader: &mut BufReader<TcpStream>) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    let n = reader.by_ref().take(LINE_MAX as u64 + 1).read_until(b'\n', &mut line).ok()?;
    if n == 0 || line.len() > LINE_MAX {
        return None;
    }
    line.pop_if(|b| *b == b'\n');
    Some(line)
}

/// Compares in the same time whatever the bytes (the token).
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

struct Loaded {
    instance: Instance,
    gate: Gate,
    path: PathBuf,
    modified: Option<SystemTime>,
    /// It failed: shown as an error until a new build.
    stopped: bool,
}

/// What the module shows, as sent: title, tree, offers, pictures.
type FrameShown = (String, Node, Vec<Offer>, BTreeMap<String, Shown>);

/// The module a window showed before another was opened over it (the git graph under
/// the diff of a commit): kept as it was, its git runs still going, until Back.
struct Under {
    module: Loaded,
    arg: Option<String>,
    open: Option<OpenTarget>,
    wants_window: bool,
    tools: Vec<Receiver<ToolResult>>,
    tool_runs: Vec<ToolResult>,
}

/// A module change the module shown asked for, made once its call is over.
enum Switch {
    /// Open this module over the one shown, with this arg.
    Over(&'static str, String),
    /// Back to the one under.
    Back,
}

struct Session<'a> {
    stream: TcpStream,
    config: &'a Config,
    module: Option<Loaded>,
    arg: Option<String>,
    width: f32,
    height: f32,
    theme: Option<Theme>,
    focused: bool,
    mods: Mods,
    /// File answers for the next call, and their bytes.
    fs: Vec<FsResult>,
    fs_bytes: Vec<u8>,
    /// Open handles, watches and picked files; the picker while it's open.
    files: Files,
    /// The desktop's file chooser while it's open: the request's id, and its answer.
    picker: Option<(String, Receiver<Picked>)>,
    fs_events: Vec<FsWatchEvent>,
    images: Images,
    /// What it showed last.
    shown: Option<FrameShown>,
    last_tick: Instant,
    /// The last `seq` received: the next answer acknowledges it.
    seq: u64,
    /// When the line being answered was read (ms since the epoch).
    got: f64,
    /// The time the last answer took in the module.
    call_ms: f64,
    /// What it was opened with.
    open: Option<OpenTarget>,
    /// Something it shows changed (a tool run answered): call it again.
    again: bool,
    /// Where its tab works, as the mod last said; and whether the module asked for it.
    window: Option<WindowContext>,
    wants_window: bool,
    /// The client draws canvases itself: big ones aren't made pictures.
    native_canvas: bool,
    /// Tool runs going, and answers for the next call.
    tools: Vec<Receiver<ToolResult>>,
    tool_runs: Vec<ToolResult>,
    /// Each canvas's zoom, from its last view (big canvases are drawn here at it).
    zooms: std::collections::HashMap<String, f32>,
    /// The pictures of big canvases sent lately, oldest first.
    drawn: std::collections::VecDeque<String>,
    /// The module under the one shown, and a change asked for during a call.
    under: Option<Box<Under>>,
    switch: Option<Switch>,
    closed: bool,
}

impl<'a> Session<'a> {
    fn new(stream: TcpStream, config: &'a Config, arg: Option<String>, images: Option<&str>) -> Self {
        Session {
            stream,
            config,
            module: None,
            arg,
            width: 0.0,
            height: 0.0,
            theme: None,
            focused: false,
            mods: Mods::default(),
            fs: Vec::new(),
            fs_bytes: Vec::new(),
            files: Files::default(),
            picker: None,
            fs_events: Vec::new(),
            images: Images::new(images),
            shown: None,
            last_tick: Instant::now(),
            seq: 0,
            got: 0.0,
            call_ms: 0.0,
            open: None,
            again: false,
            window: None,
            wants_window: false,
            native_canvas: false,
            tools: Vec::new(),
            tool_runs: Vec::new(),
            zooms: Default::default(),
            drawn: Default::default(),
            under: None,
            switch: None,
            closed: false,
        }
    }

    /// What the user opened the module with: a link, or a file it may then read (as if
    /// picked).
    fn opened(&mut self, open: Option<&str>) {
        let Some(open) = open.map(str::trim).filter(|o| !o.is_empty()) else { return };
        if open.starts_with("https://") || open.starts_with("http://") {
            self.open = Some(OpenTarget::Url(open.into()));
            return;
        }
        let home = crate::fs::home();
        let Ok(path) = crate::fs::expand(open, home.as_deref()) else { return };
        let real = path.canonicalize().unwrap_or(path);
        if let (Some(dir), Some(name)) = (real.parent(), real.file_name()) {
            self.files.picked.push(crate::fs::Root { dir: dir.to_owned(), only: Some(name.to_owned()), read: true, write: false });
            let path = crate::fs::display(&real, home.as_deref());
            self.open = Some(OpenTarget::File { path, line: None, col: None });
        }
    }

    fn send(&mut self, out: &Out) {
        if self.stream.write_all(&wire::line(out)).is_err() {
            self.closed = true;
        }
    }

    fn path_of(&self, name: &str) -> Option<PathBuf> {
        wire::module_name_ok(name).then(|| self.config.modules.join(format!("{name}.wasm")))
    }

    fn load(&mut self, name: &str) -> Result<(), String> {
        let path = self.path_of(name).ok_or_else(|| format!("{name:?} isn't a module name (a-z 0-9 - _)"))?;
        let t = Instant::now();
        let loaded = load_at(name, &path)?;
        if crate::extensions::is_off(name) {
            return Err(format!("{name} is turned off: turn it back on in WASM permissions (or wasm-mods-host --enable)"));
        }
        let load_ms = t.elapsed().as_secs_f64() * 1e3;
        let description = loaded.gate.manifest.as_ref().map(|m| m.description.clone()).unwrap_or_default();
        self.module = Some(loaded);
        self.send(&Out::Ready { name, description: &description, load_ms });
        Ok(())
    }

    fn handle(&mut self, msg: In) {
        match msg {
            In::Hello { .. } => {}
            In::Size { width, height } => {
                self.width = width;
                self.height = height;
                self.render(Vec::new());
            }
            In::Theme(theme) => {
                self.theme = Some(theme);
                self.render(Vec::new());
            }
            In::Focused(f) => {
                self.focused = f;
            }
            In::Window(window) => {
                let changed = self.window != window;
                self.window = window;
                if changed && self.wants_window {
                    self.render(Vec::new());
                }
            }
            In::Events { seq, events, mods } => {
                self.seq = seq;
                self.mods = mods;
                self.render(events);
            }
        }
    }

    /// Watched files are looked at; once a second a new build of the module loads in
    /// place.
    fn tick(&mut self) {
        // The user answered the file chooser.
        if let Some((id, rx)) = &self.picker {
            let got = match rx.try_recv() {
                Ok(picked) => Some(picked),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(_) => Some(Picked { reply: FsReply::Error("the file chooser went away".into()), root: None }),
            };
            if let Some(picked) = got {
                let id = id.clone();
                self.picker = None;
                if let Some(root) = picked.root {
                    self.files.picked.push(root);
                }
                self.fs.push(FsResult { id, reply: picked.reply });
                self.render(Vec::new());
            }
        }
        // Tool runs that finished.
        let before = self.tool_runs.len();
        self.tools.retain(|rx| match rx.try_recv() {
            Ok(result) => {
                self.tool_runs.push(result);
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(_) => false,
        });
        if self.tool_runs.len() > before {
            self.render(Vec::new());
        }
        if self.files.watching()
            && let Some(m) = &mut self.module
        {
            let roots = self.files.roots(&crate::fs::roots(&mut m.gate));
            let events = self.files.poll(&roots);
            if !events.is_empty() {
                self.fs_events.extend(events);
                self.render(Vec::new());
            }
        }
        if self.last_tick.elapsed() < TICK {
            return;
        }
        self.last_tick = Instant::now();
        self.reload_if_changed();
    }

    fn reload_if_changed(&mut self) {
        let Some(m) = &mut self.module else { return };
        let modified = std::fs::metadata(&m.path).and_then(|md| md.modified()).ok();
        if modified == m.modified || modified.is_none() {
            return;
        }
        let name = m.gate.id.name.clone();
        let state = (!m.stopped).then(|| m.instance.save().ok()).flatten();
        match load_at(&name, &m.path.clone()) {
            Ok(mut new) => {
                if let Some(state) = state {
                    let _ = new.instance.restore(&state);
                }
                *m = new;
                eprintln!("wasm-mods: {name} reloaded");
            }
            Err(e) => {
                m.modified = modified;
                m.stopped = true;
                self.send(&Out::Error { message: &e });
                return;
            }
        }
        self.render(Vec::new());
    }

    /// Calls the module with `events` (raw, as they came) and sends what it shows.
    fn render(&mut self, events: Vec<serde_json::Value>) {
        let Some(theme) = self.theme.clone() else { return };
        let Some(m) = &mut self.module else { return };
        if m.stopped {
            return;
        }
        if self.width <= 0.0 {
            return;
        }
        let parsed: Vec<Event> = events.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect();
        for e in &parsed {
            if let Event::View { id, zoom, .. } = e {
                self.zooms.insert(id.clone(), *zoom);
            }
        }
        // Nothing runs before the user answered for every capability it declared.
        let mut pending = m.gate.pending();
        if !pending.is_empty() {
            if let Some(allow) = permissions::prompt_answer(&parsed) {
                m.gate.answer(allow);
                pending = m.gate.pending();
            }
            if !pending.is_empty() {
                let ui = permissions::prompt(&m.gate, &pending);
                let ack = self.seq;
                let got = self.got;
                self.send(&Out::Frame { title: "", ui: &ui, actions: &[], ack, call_ms: 0.0, got, sent: wire::now_ms(), images: &BTreeMap::new() });
                return;
            }
            // The prompt's own clicks aren't the module's.
            return self.render(Vec::new());
        }
        self.call(parsed, theme)
    }

    fn call(&mut self, mut events: Vec<Event>, theme: Theme) {
        let t = Instant::now();
        // A link opens only in answer to the user's own click or key.
        let gesture = events.iter().any(|e| {
            matches!(e, Event::Clicked { .. } | Event::Activated { .. } | Event::Submitted { .. } | Event::Action(_))
        });
        for _ in 0..CHAIN_MAX {
            let Some(m) = &mut self.module else { return };
            events.append(&mut self.images.events);
            let input = Input {
                time: self.config.started.elapsed().as_secs_f64(),
                width: self.width,
                height: self.height,
                focused: self.focused,
                theme: theme.clone(),
                events: std::mem::take(&mut events),
                data: HostData {
                    fs: std::mem::take(&mut self.fs),
                    fs_events: std::mem::take(&mut self.fs_events),
                    tool_runs: std::mem::take(&mut self.tool_runs),
                },
                window: if self.wants_window && m.gate.allows(&Capability::WindowFolder) { self.window.clone() } else { None },
                arg: self.arg.clone(),
                open: self.open.clone(),
                opened_from: self.under.as_ref().map(|u| u.module.gate.id.name.clone()),
                mods: self.mods,
                ..Default::default()
            };
            let json = serde_json::to_vec(&input).unwrap_or_default();
            let side = std::mem::take(&mut self.fs_bytes);
            let answer = m.instance.call(&json, &side).map_err(|e| e.0).and_then(|(json, bytes)| {
                let mut out: Output = serde_json::from_slice(&json).map_err(|e| format!("its answer: {e}"))?;
                out.bytes = bytes;
                Ok(out)
            });
            let out = match answer {
                Ok(out) => out,
                Err(e) => return self.stop(&e),
            };
            // Asked for its tab's folder just now: called again with it.
            if out.wants.window && !self.wants_window && self.window.is_some() {
                self.again = true;
            }
            self.wants_window = out.wants.window;
            self.effects(&out, gesture);
            // Another module in this window (or back to the one under): it's called
            // instead, and this answer isn't shown.
            if let Some(switch) = self.switch.take() {
                match switch {
                    Switch::Over(name, arg) => self.open_over(name, arg),
                    Switch::Back => self.back(),
                }
                return self.render(Vec::new());
            }
            let Some(m) = &mut self.module else { return };
            let roots = self.files.roots(&crate::fs::roots(&mut m.gate));
            let mut images = self.images.resolve(&out.ui, &out.bytes, &self.files, &roots);
            let mut ui = out.ui;
            if let Some(dir) = self.images.dir().filter(|_| !self.native_canvas) {
                let drawn = crate::canvas::flatten(&mut ui, dir, &self.zooms);
                // Each change is a new picture: the last few are kept (one may still be
                // on screen), older ones deleted.
                for shown in drawn.values() {
                    if !self.drawn.contains(&shown.file) {
                        self.drawn.push_back(shown.file.clone());
                    }
                }
                while self.drawn.len() > 3 * drawn.len().max(1) {
                    if let Some(old) = self.drawn.pop_front() {
                        let _ = std::fs::remove_file(dir.join(old));
                    }
                }
                images.extend(drawn);
            }
            self.shown = Some((out.title, ui, out.actions, images));
            self.call_ms = t.elapsed().as_secs_f64() * 1e3;
            self.resend();
            if self.fs.is_empty() && self.images.events.is_empty() && !std::mem::take(&mut self.again) {
                return;
            }
        }
    }

    /// Opens `name` (with `arg`) in this window over the module shown, which is kept
    /// as it is under it. If it can't load, it opens in a tab of its own instead.
    fn open_over(&mut self, name: &'static str, arg: String) {
        let loaded = self.path_of(name).ok_or_else(|| format!("{name:?} isn't a module name")).and_then(|path| load_at(name, &path));
        let loaded = match loaded {
            Ok(_) if crate::extensions::is_off(name) => Err(format!("{name} is turned off")),
            other => other,
        };
        let new = match loaded {
            Ok(new) => new,
            Err(e) => {
                eprintln!("wasm-mods: {name} in this window: {e}; in a tab of its own");
                self.send(&Out::Open { module: name, arg: &arg });
                return;
            }
        };
        let Some(module) = self.module.replace(new) else { return };
        self.under = Some(Box::new(Under {
            module,
            arg: self.arg.replace(arg),
            open: self.open.take(),
            wants_window: std::mem::replace(&mut self.wants_window, true),
            tools: std::mem::take(&mut self.tools),
            tool_runs: std::mem::take(&mut self.tool_runs),
        }));
        self.shown = None;
        self.ready();
    }

    /// Back to the module under the one shown, as it was; the one shown is dropped.
    fn back(&mut self) {
        let Some(u) = self.under.take() else { return };
        self.module = Some(u.module);
        self.arg = u.arg;
        self.open = u.open;
        self.wants_window = u.wants_window;
        self.tools = u.tools;
        self.tool_runs = u.tool_runs;
        self.shown = None;
        self.ready();
    }

    /// Tells the client which module it now shows.
    fn ready(&mut self) {
        let Some(m) = &self.module else { return };
        let name = m.gate.id.name.clone();
        let description = m.gate.manifest.as_ref().map(|m| m.description.clone()).unwrap_or_default();
        self.send(&Out::Ready { name: &name, description: &description, load_ms: 0.0 });
    }

    /// Sends what it showed last again.
    fn resend(&mut self) {
        let Some((title, ui, actions, images)) = &self.shown else { return };
        let (ack, call_ms, got) = (self.seq, self.call_ms, self.got);
        let line = wire::line(&Out::Frame { title, ui, actions, ack, call_ms, got, sent: wire::now_ms(), images });
        if self.stream.write_all(&line).is_err() {
            self.closed = true;
        }
    }

    /// Carries out what the module asked for. Files answer in the next call; effects that
    /// have no meaning in herdr-app are dropped.
    fn effects(&mut self, out: &Output, gesture: bool) {
        let Some(m) = &mut self.module else { return };
        let mut roots: Option<Vec<crate::fs::Root>> = None;
        let mut requests = 0;
        let mut lines = Vec::new();
        for effect in &out.effects {
            match effect {
                Effect::Fs { id, op } => {
                    requests += 1;
                    if let Some(ask) = portal::Ask::of(op) {
                        // Only in answer to the user's own click or key: a module can't
                        // pop dialogs up by itself.
                        let reply = if self.picker.is_some() {
                            Some("a file chooser is open already")
                        } else if !gesture {
                            Some("a file chooser opens only when the user asks (a click or a key)")
                        } else {
                            None
                        };
                        match reply {
                            Some(e) => self.fs.push(FsResult { id: id.clone(), reply: FsReply::Error(e.into()) }),
                            None => self.picker = Some((id.clone(), portal::pick(&m.gate.id.name, ask))),
                        }
                        continue;
                    }
                    let (reply, mut bytes) = if requests > crate::fs::REQUESTS_MAX {
                        let e = format!("too many file requests in one call (at most {})", crate::fs::REQUESTS_MAX);
                        (FsReply::Error(e), Vec::new())
                    } else {
                        let roots = roots.get_or_insert_with(|| self.files.roots(&crate::fs::roots(&mut m.gate)));
                        match self.files.run(id, op, roots) {
                            Some(answer) => answer,
                            None => crate::fs::run(op, &out.bytes, roots),
                        }
                    };
                    let reply = match reply {
                        wasm_mods_protocol::FsReply::Bytes { offset, data, eof } => {
                            let data = wasm_mods_protocol::Blob { start: self.fs_bytes.len() as u32, len: data.len };
                            wasm_mods_protocol::FsReply::Bytes { offset, data, eof }
                        }
                        other => other,
                    };
                    self.fs_bytes.append(&mut bytes);
                    self.fs.push(FsResult { id: id.clone(), reply });
                }
                Effect::RunTool { tool, id, args } => {
                    let refused = |why: String| ToolResult { tool: tool.clone(), id: id.clone(), status: None, stdout: String::new(), stderr: why };
                    // On this computer, or (a space on a saved machine) on that one.
                    let place = self.window.as_ref().map(|w| match &w.machine {
                        None => crate::fs::expand(&w.folder, crate::fs::home().as_deref()).map(crate::tools::Place::Here),
                        Some(machine) if w.folder.contains('\0') => Err(format!("not a valid folder on {machine}")),
                        Some(machine) => Ok(crate::tools::Place::On { machine: machine.clone(), folder: w.folder.clone() }),
                    });
                    let why = if tool != "git" {
                        Some(format!("{tool} isn't available here (only git)"))
                    } else if !m.gate.allows(&Capability::Git) {
                        Some("git isn't allowed for this module".into())
                    } else if self.tools.len() + self.under.as_ref().map_or(0, |u| u.tools.len()) >= crate::tools::RUNNING_MAX {
                        Some(format!("at most {} runs at once", crate::tools::RUNNING_MAX))
                    } else {
                        None
                    };
                    match (why, place) {
                        (Some(why), _) => self.tool_runs.push(refused(why)),
                        (None, Some(Ok(place))) => self.tools.push(crate::tools::run_git(id.clone(), place, args.clone())),
                        (None, Some(Err(why))) => self.tool_runs.push(refused(format!("no folder: {why}"))),
                        (None, None) => self.tool_runs.push(refused("no folder: its tab has no terminal in a folder".into())),
                    }
                    self.again |= !self.tool_runs.is_empty();
                }
                // The diff of a commit, in this window over the graph: Esc there comes back.
                Effect::OpenDiff { commit } if gesture && self.under.is_none() => {
                    self.switch = Some(Switch::Over("diff", commit.clone()));
                }
                Effect::OpenDiff { commit } if gesture => {
                    lines.push(wire::line(&Out::Open { module: "diff", arg: commit }));
                }
                Effect::Focus { id } => lines.push(wire::line(&Out::Focus { id })),
                Effect::Close if self.under.is_some() => self.switch = Some(Switch::Back),
                Effect::Close => lines.push(wire::line(&Out::Close)),
                Effect::OpenUrl { url } if gesture && (url.starts_with("https://") || url.starts_with("http://")) => {
                    lines.push(wire::line(&Out::OpenUrl { url }))
                }
                other => eprintln!("wasm-mods: {} asked for {}, which herdr-app doesn't do; dropped", m.gate.id.name, effect_name(other)),
            }
        }
        for line in lines {
            if self.stream.write_all(&line).is_err() {
                self.closed = true;
            }
        }
    }

    fn stop(&mut self, why: &str) {
        let name = self.module.as_ref().map(|m| m.gate.id.name.clone()).unwrap_or_default();
        if let Some(m) = &mut self.module {
            m.stopped = true;
        }
        let message = format!("{name} stopped: {why}. A new build of it starts it again.");
        self.send(&Out::Error { message: &message });
    }
}

fn load_at(name: &str, path: &Path) -> Result<Loaded, String> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let bytes = std::fs::read(path).map_err(|e| format!("no module {name} ({}: {e})", path.display()))?;
    let hash = permissions::hash(&bytes);
    let instance = Instance::load(&bytes, &hash).map_err(|e| format!("{name}: {e}"))?;
    let gate = Gate::new(ModuleId { name: name.into() }, &bytes);
    if gate.manifest.is_none() {
        eprintln!("wasm-mods: {name} has no manifest: it gets no capabilities");
    }
    Ok(Loaded { instance, gate, path: path.to_owned(), modified, stopped: false })
}

/// An effect's name, for the log.
fn effect_name(e: &Effect) -> String {
    let debug = format!("{e:?}");
    debug.split(|c: char| !c.is_alphanumeric()).next().unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_must_match_exactly() {
        assert!(same(b"abc123", b"abc123"));
        for bad in [&b"abc124"[..], b"abc12", b"abc1234", b""] {
            assert!(!same(bad, b"abc123"));
        }
    }
}
