//! Write a WASM mod: a module the WASM Mods host runs in a Herdr pane.
//!
//! ```ignore
//! use wasm_mods_sdk::*;
//!
//! #[derive(Default, serde::Serialize, serde::Deserialize)]
//! struct Counter { clicks: u32 }
//!
//! impl Frame for Counter {
//!     fn update(&mut self, input: &Input) -> Output {
//!         for event in &input.events {
//!             if let Event::Clicked { id } = event && id == "more" { self.clicks += 1; }
//!         }
//!         Output::new("counter", column([
//!             text(format!("{} clicks", self.clicks)),
//!             button("more", "click me"),
//!         ]))
//!     }
//! }
//!
//! export_frame!(Counter);
//! manifest!(description: "a counter", capabilities: []);
//! ```
//!
//! Build with `cargo build --release --target wasm32-unknown-unknown`. The host runs the
//! module with wasmtime and calls `update` whenever something happened: the pane's size
//! or theme changed, the user did something, or an answer it asked for came.
//!
//! - Take every colour from `input.theme`, so themes apply inside the module.
//! - The module's state (your struct) is serialized across live reloads, so a rebuilt
//!   module keeps where it was. Fields you add later should have defaults
//!   (`#[serde(default)]`).
//! - `update` is pure: no files, network or clocks. Ask for host data with
//!   [`Output::wants`] and act through [`Output::effects`].
//!
//! # Components
//!
//! The UI is a tree of eight components ([`Node`]: `Column`, `Row`, `Text`, `TextField`,
//! `List`, `Canvas`, `Popup`, `Empty`) with shared style and layout props. Build them
//! with the helpers re-exported from [`wasm_mods_protocol::ui`]: `column`, `row`, `text`,
//! `heading`, `button`, `primary`, `toggle`, `checkbox`, `tile`, `progress`, `menu`,
//! `dialog`, and modifiers like
//! `.id(…)` (pressable), `.tooltip(…)`, `.width(Size::Fill)`. Text fields keep their
//! text in a [`FieldState`]. A `list` keeps its selection from [`Event::Selected`]
//! (the host moves it; open is [`Event::Activated`]); its rows can hold any node
//! (`row_item`). Lists, tables and text never go on a canvas: it's for drawings.
//!
//! # Actions, not keys
//!
//! A module never sees a key. It gets [`Event::Action`]: [`Action::Back`],
//! [`Action::Open`], [`Action::Primary`], [`Action::Secondary`] and [`Action::Move`],
//! from the keyboard, a gamepad or a click on the hints the host draws under the window,
//! and it says which it offers, labelled, in [`Output::actions`] (`.action(…)`); the host
//! draws those hints in the glyphs of the device the user last used. Don't draw key
//! hints yourself. Up to [`MAX_KEYS`] keys of its own ([`Output::key`]: a plain key
//! such as `"n"`, never one the host keeps for the actions) arrive as [`Action::Key`]
//! while its pane has the keyboard.
//!
//! Lay the UI out top to bottom at its natural height: the host scrolls the window when
//! it's taller, so don't size content to `input.height` or build your own scrolling.
//! Only a canvas with [`Node::pan_zoom`] (the host pans and zooms it; [`Event::View`]) and a
//! list with virtual rows ([`VirtualRows`], [`Event::VisibleRange`]) move on their own.
//! Pictures are canvas shapes ([`image`], [`zoomable_image`], [`ImageSrc`]).
//!
//! # Permissions
//!
//! Every module declares what it needs with [`manifest!`]; the host reads it before running
//! the module, asks the user about anything not yet allowed, and only hands over data
//! (and carries out effects) for granted capabilities:
//!
//! ```ignore
//! manifest!(description: "a list of things to do", capabilities: ["fs.read:~/todo", "fs.write:~/todo"]);
//! ```
//!
//! | capability | lets the module |
//! |---|---|
//! | `window.folder` | know the folder and git branch of the tab it's shown in ([`Wants::window`], [`Input::window`]); open the host's diff viewer there ([`Effect::OpenDiff`]) |
//! | `git` | run git as the user in that folder, only commands that read ([`run_tool`]`("git", …)`, results in [`HostData::tool_runs`]) |
//! | `fs.read:<dir>` | read, list, stat and watch files inside `<dir>` (`~/notes`, `/srv/logs`) through [`fs`] |
//! | `fs.write:<dir>` | write, make folders and delete inside `<dir>` through [`fs`] |
//!
//! Each is asked once per build: a new build of the module asks again.
//!
//! A tool command whose capability isn't granted, or a tool the host doesn't run, still gets
//! an answer: a [`ToolResult`] with no `status` and the reason in `stderr` (so is a
//! missing binary: "git is not installed").
//!
//! A module declaring `window.folder` opened in a tab gets that tab's folder; opened
//! with no folder it runs on its own (no [`Input::window`]). A tab on one of Herdr's saved machines sets
//! [`WindowContext::machine`]: the folder is on that machine, and git runs there (with
//! the same checks).
//!
//! Without any `fs` capability a module can still let the user choose: [`fs::pick_file`],
//! [`fs::pick_folder`] and [`fs::pick_save`] open the host's own picker, and what the user
//! picks becomes readable (a file, a folder) or writable (a save location) for that module.
//!
//! A capability this host doesn't know is never granted and never asked.

pub use wasm_mods_protocol::*;

/// A module's state and logic: the host calls `update` with what happened, and draws
/// and carries out what it returns.
pub trait Frame: Default + serde::Serialize + serde::de::DeserializeOwned {
    fn update(&mut self, input: &Input) -> Output;
}

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------
//
// The component builders (`column`, `row`, `text`, `heading`, `button`, `primary`,
// `tile`, `list`, `menu`, `dialog`, `progress`, `FieldState`, `Fuzzy`, …)
// live in the protocol's `ui` module, shared with the host's Allow prompt; they're re-exported
// here, so `use wasm_mods_sdk::*` brings them in.

pub use wasm_mods_protocol::ui::*;

/// `0xRRGGBBAA` with a new alpha.
pub fn with_alpha(color: Color, alpha: u8) -> Color {
    (color & 0xffff_ff00) | alpha as u32
}

// ---------------------------------------------------------------------------
// Command-line tools
// ---------------------------------------------------------------------------

/// Runs `tool args` (`git`, read-only commands) in the tab's folder; the answer comes
/// back in a later call's [`HostData::tool_runs`] under `id` (see [`tool_result`]).
/// Declare the capabilities: `manifest!(capabilities: ["window.folder", "git"])`.
pub fn run_tool<'a>(tool: &str, id: impl Into<String>, args: impl IntoIterator<Item = &'a str>) -> Effect {
    Effect::RunTool { tool: tool.into(), id: id.into(), args: args.into_iter().map(str::to_owned).collect() }
}

/// The answer to the [`run_tool`] of `tool` with `id` in this call's host data, if it came.
pub fn tool_result<'a>(data: &'a HostData, tool: &str, id: &str) -> Option<&'a ToolResult> {
    data.tool_runs.iter().find(|r| r.tool == tool && r.id == id)
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// Files, through the host: each request is an [`Effect`] with an `id` of your choosing; the
/// answer comes in a later call ([`Input::fs_result`]), with any bytes beside it
/// ([`Input::blob`]). Paths are absolute or `~/…`, inside a folder the module was granted
/// (`fs.read:<dir>`, `fs.write:<dir>`) or something the user picked for it.
///
/// ```ignore
/// // once:            out.effects.push(fs::open("log", "~/logs/build.log"));
/// // when it's open:  if let Some(FsReply::Opened { handle, stat }) = input.fs_result("log") { … }
/// // a page:          out.effects.push(fs::read_range("page", handle, offset, 64 * 1024));
/// // its bytes:       if let Some(FsReply::Bytes { data, .. }) = input.fs_result("page") { input.blob(*data) }
/// ```
pub mod fs {
    use super::*;

    fn op(id: impl Into<String>, op: FsOp) -> Effect {
        Effect::Fs { id: id.into(), op }
    }

    /// Opens a file for [`read_range`] and [`read_lines`]: [`FsReply::Opened`].
    pub fn open(id: impl Into<String>, path: impl Into<String>) -> Effect {
        op(id, FsOp::Open { path: path.into() })
    }

    pub fn close(id: impl Into<String>, handle: u64) -> Effect {
        op(id, FsOp::Close { handle })
    }

    pub fn stat(id: impl Into<String>, path: impl Into<String>) -> Effect {
        op(id, FsOp::Stat { path: path.into() })
    }

    pub fn stat_handle(id: impl Into<String>, handle: u64) -> Effect {
        op(id, FsOp::StatHandle { handle })
    }

    /// Up to `len` bytes (at most 4 MB) from `offset`: [`FsReply::Bytes`].
    pub fn read_range(id: impl Into<String>, handle: u64, offset: u64, len: u32) -> Effect {
        op(id, FsOp::ReadRange { handle, offset, len })
    }

    /// `count` lines (at most 2000) from line `start`: [`FsReply::Lines`]. Ask again while
    /// it answers with no lines and `indexed < 1`.
    pub fn read_lines(id: impl Into<String>, handle: u64, start: u64, count: u32) -> Effect {
        op(id, FsOp::ReadLines { handle, start, count })
    }

    /// A whole file of at most 8 MB: [`FsReply::Bytes`].
    pub fn read_file(id: impl Into<String>, path: impl Into<String>) -> Effect {
        op(id, FsOp::ReadFile { path: path.into() })
    }

    /// A folder's entries: [`FsReply::Entries`].
    pub fn list(id: impl Into<String>, path: impl Into<String>) -> Effect {
        op(id, FsOp::List { path: path.into() })
    }

    /// Replaces `path` with `data` (at most 8 MB), atomically: adds the request to `out`.
    pub fn write(out: &mut Output, id: impl Into<String>, path: impl Into<String>, data: &[u8]) {
        let data = out.add_bytes(data);
        out.effects.push(op(id, FsOp::Write { path: path.into(), data }));
    }

    pub fn create_dir(id: impl Into<String>, path: impl Into<String>) -> Effect {
        op(id, FsOp::CreateDir { path: path.into() })
    }

    /// Deletes a file or an empty folder.
    pub fn delete(id: impl Into<String>, path: impl Into<String>) -> Effect {
        op(id, FsOp::Delete { path: path.into() })
    }

    /// Watches a file or folder: [`FsReply::Watching`], then [`HostData::fs_events`]
    /// under `id` until [`unwatch`]`(id)`.
    pub fn watch(id: impl Into<String>, path: impl Into<String>) -> Effect {
        op(id, FsOp::Watch { path: path.into() })
    }

    pub fn watch_handle(id: impl Into<String>, handle: u64) -> Effect {
        op(id, FsOp::WatchHandle { handle })
    }

    pub fn unwatch(id: impl Into<String>) -> Effect {
        op(id, FsOp::Unwatch)
    }

    /// The host's file picker; `extensions` narrow the list (`["md", "txt"]`).
    pub fn pick_file<'a>(id: impl Into<String>, extensions: impl IntoIterator<Item = &'a str>) -> Effect {
        op(id, FsOp::PickFile { extensions: extensions.into_iter().map(str::to_owned).collect() })
    }

    pub fn pick_folder(id: impl Into<String>) -> Effect {
        op(id, FsOp::PickFolder)
    }

    /// Where to save one file, suggesting `name`.
    pub fn pick_save(id: impl Into<String>, name: impl Into<String>) -> Effect {
        op(id, FsOp::PickSave { suggested: name.into() })
    }

    /// The events of the watch `id` in this call.
    pub fn events<'a>(input: &'a Input, id: &'a str) -> impl Iterator<Item = &'a FsChange> + 'a {
        input.data.fs_events.iter().filter(move |e| e.id == id).map(|e| &e.change)
    }
}

// ---------------------------------------------------------------------------
// The WebAssembly boundary
// ---------------------------------------------------------------------------

/// Implementation details of [`export_frame!`].
#[doc(hidden)]
pub mod __abi {
    use super::*;
    pub use serde_json;

    /// A buffer handed to the host: `(ptr << 32) | len`. The host frees it with
    /// `wm_free(ptr, len)`.
    pub fn leak(bytes: Vec<u8>) -> u64 {
        let mut bytes = bytes.into_boxed_slice();
        let (ptr, len) = (bytes.as_mut_ptr() as usize as u64, bytes.len() as u64);
        std::mem::forget(bytes);
        (ptr << 32) | len
    }

    /// Takes ownership of a buffer the host wrote after `wm_alloc`.
    ///
    /// # Safety
    /// `ptr`/`len` must come from `wm_alloc(len)`.
    pub unsafe fn take(ptr: u32, len: u32) -> Vec<u8> {
        unsafe { Vec::from_raw_parts(ptr as usize as *mut u8, len as usize, len as usize) }
    }

    pub fn alloc(len: u32) -> u32 {
        let mut buf = Vec::<u8>::with_capacity(len as usize);
        let ptr = buf.as_mut_ptr();
        std::mem::forget(buf);
        ptr as usize as u32
    }

    /// `s` as a byte array, for the manifest section.
    pub const fn manifest_bytes<const N: usize>(s: &str) -> [u8; N] {
        let bytes = s.as_bytes();
        let mut out = [0u8; N];
        let mut i = 0;
        while i < N {
            out[i] = bytes[i];
            i += 1;
        }
        out
    }

    pub fn update<F: Frame>(state: &mut F, input: &[u8]) -> Vec<u8> {
        let output = match serde_json::from_slice::<Input>(input) {
            Ok(input) => state.update(&input),
            Err(e) => Output::new("module error", styled(format!("bad input from the host: {e}"), TextStyle::Accent)),
        };
        serde_json::to_vec(&output).unwrap_or_default()
    }

    /// `wm_update_bytes`: the input JSON plus its raw bytes ([`Input::bytes`]) in; out,
    /// `[json length: u32 LE][output JSON][output bytes]` ([`Output::bytes`]).
    pub fn update_bytes<F: Frame>(state: &mut F, input: &[u8], bytes: Vec<u8>) -> Vec<u8> {
        let output = match serde_json::from_slice::<Input>(input) {
            Ok(mut input) => {
                input.bytes = bytes;
                state.update(&input)
            }
            Err(e) => Output::new("module error", styled(format!("bad input from the host: {e}"), TextStyle::Accent)),
        };
        let json = serde_json::to_vec(&output).unwrap_or_default();
        let mut out = Vec::with_capacity(4 + json.len() + output.bytes.len());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(&json);
        out.extend_from_slice(&output.bytes);
        out
    }
}

/// Exports `T: Frame` as the module (the WebAssembly ABI the host calls).
#[macro_export]
macro_rules! export_frame {
    ($frame:ty) => {
        ::std::thread_local! {
            static __WM_FRAME: ::std::cell::RefCell<$frame> =
                ::std::cell::RefCell::new(<$frame as ::core::default::Default>::default());
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn wm_protocol_version() -> u32 {
            $crate::PROTOCOL_VERSION
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn wm_alloc(len: u32) -> u32 {
            $crate::__abi::alloc(len)
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn wm_free(ptr: u32, len: u32) {
            drop(unsafe { $crate::__abi::take(ptr, len) });
        }

        /// Input JSON in, `(ptr << 32) | len` of the output JSON out.
        #[unsafe(no_mangle)]
        pub extern "C" fn wm_update(ptr: u32, len: u32) -> u64 {
            let input = unsafe { $crate::__abi::take(ptr, len) };
            let out = __WM_FRAME.with(|f| $crate::__abi::update(&mut *f.borrow_mut(), &input));
            $crate::__abi::leak(out)
        }

        /// The same with raw bytes beside the JSON both ways (file contents): what the host
        /// calls when a module exports it.
        #[unsafe(no_mangle)]
        pub extern "C" fn wm_update_bytes(ptr: u32, len: u32, bytes_ptr: u32, bytes_len: u32) -> u64 {
            let input = unsafe { $crate::__abi::take(ptr, len) };
            let bytes = unsafe { $crate::__abi::take(bytes_ptr, bytes_len) };
            let out = __WM_FRAME.with(|f| $crate::__abi::update_bytes(&mut *f.borrow_mut(), &input, bytes));
            $crate::__abi::leak(out)
        }

        /// The module's state as JSON, kept across live reloads.
        #[unsafe(no_mangle)]
        pub extern "C" fn wm_save() -> u64 {
            let out = __WM_FRAME.with(|f| $crate::__abi::serde_json::to_vec(&*f.borrow()).unwrap_or_default());
            $crate::__abi::leak(out)
        }

        /// Restores state saved by an earlier build; ignored if it no longer fits.
        #[unsafe(no_mangle)]
        pub extern "C" fn wm_restore(ptr: u32, len: u32) {
            let saved = unsafe { $crate::__abi::take(ptr, len) };
            if let Ok(state) = $crate::__abi::serde_json::from_slice::<$frame>(&saved) {
                __WM_FRAME.with(|f| *f.borrow_mut() = state);
            }
        }
    };
}

/// Declares what the module is and the capabilities it needs (see the crate docs), as
/// the custom wasm section `wasm_mods.manifest` the host reads before running it. The name and
/// version are the crate's.
#[macro_export]
macro_rules! manifest {
    (description: $description:literal, capabilities: [$($capability:literal),* $(,)?] $(,)?) => {
        const __WM_MANIFEST_TEXT: &str = concat!(
            "name=", env!("CARGO_PKG_NAME"), "\n",
            "description=", $description, "\n",
            "version=", env!("CARGO_PKG_VERSION"), "\n",
            // The protocol it's built for (PROTOCOL_VERSION): the host doesn't run builds
            // for another.
            "protocol=5\n",
            $("capability=", $capability, "\n",)*
        );
        #[unsafe(link_section = "wasm_mods.manifest")]
        #[used]
        static __WM_MANIFEST: [u8; __WM_MANIFEST_TEXT.len()] =
            $crate::__abi::manifest_bytes(__WM_MANIFEST_TEXT);
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default, serde::Serialize, serde::Deserialize)]
    struct Echo;

    impl Frame for Echo {
        fn update(&mut self, input: &Input) -> Output {
            let mut out = Output::new("echo", Node::Empty);
            if let Some(FsReply::Bytes { data, .. }) = input.fs_result("r") {
                let echoed = input.blob(*data).to_vec();
                fs::write(&mut out, "w", "~/notes/copy", &echoed);
            }
            out
        }
    }

    #[test]
    fn bytes_go_beside_the_json_both_ways() {
        let input = Input {
            data: HostData {
                fs: vec![FsResult { id: "r".into(), reply: FsReply::Bytes { offset: 0, data: Blob { start: 2, len: 3 }, eof: true } }],
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_vec(&input).unwrap();
        let out = __abi::update_bytes(&mut Echo, &json, b"..abc..".to_vec());
        let n = u32::from_le_bytes(out[..4].try_into().unwrap()) as usize;
        let output: Output = serde_json::from_slice(&out[4..4 + n]).unwrap();
        assert_eq!(&out[4 + n..], b"abc");
        assert_eq!(
            output.effects,
            [Effect::Fs { id: "w".into(), op: FsOp::Write { path: "~/notes/copy".into(), data: Blob { start: 0, len: 3 } } }]
        );
    }

}

#[cfg(test)]
mod manifest_protocol {
    /// `manifest!` writes the protocol as text (a macro can't put a const in `concat!`):
    /// it must be this SDK's.
    #[test]
    fn is_this_protocol() {
        let line = format!("\"protocol={}\\n\"", crate::PROTOCOL_VERSION);
        assert!(include_str!("lib.rs").contains(&line), "manifest! says another protocol than {line}");
    }
}
