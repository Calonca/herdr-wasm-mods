//! The wire: what a client (the `wasm-mods` JavaScript mod) and `wasm-mods-host` say to
//! each other. One JSON object per line over a TCP connection to 127.0.0.1 (the host's
//! port), one connection per module shown.
//!
//! 1. The client's first line is [`In::Hello`] with the host's token; anything else (or
//!    a wrong token) closes the connection.
//! 2. The host answers [`Message::Ready`] (or [`Message::Error`] and closes).
//! 3. The client sends [`In::Size`] and [`In::Theme`], then [`In::Focused`],
//!    [`In::Window`] and [`In::Events`] as they happen. The host answers with what to
//!    draw ([`Message::Frame`]) and what to do (the other [`Message`]s).
//!
//! The host also sends a frame by itself when the module has something new (a file it
//! watches changed, a picture loaded, a chooser answered).
//!
//! Externally tagged, snake case: `{"hello":{"token":"…","module":"todo"}}`,
//! `{"size":{"width":800,"height":600}}`, `"close"`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Mods, Node, Offer, Theme, WindowContext};

/// Client → host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum In {
    /// Run module `module` (a name in the modules folder: [`module_name_ok`]). `arg` is
    /// what the pane was opened for (`Input::arg`).
    Hello {
        token: String,
        module: String,
        #[serde(default)]
        arg: Option<String>,
        /// Where pictures go so the client can load them (`<app>/data/wasm-mods/images`;
        /// it must end in `wasm-mods/images`). Without it, no pictures.
        #[serde(default)]
        images: Option<String>,
        /// What the user opened it with: a file (absolute or `~/`, which it may then
        /// read) or an `http(s)` link ([`crate::Input::open`]).
        #[serde(default)]
        open: Option<String>,
        /// The client draws canvases itself, shape by shape: the host sends their shapes
        /// as they are instead of drawing big ones into a picture.
        #[serde(default)]
        native_canvas: bool,
    },
    /// The room the module has, in logical pixels.
    Size { width: f32, height: f32 },
    Theme(Theme),
    Focused(bool),
    /// Where the module's tab works ([`crate::Input::window`]): the folder of the
    /// tab's first terminal (else the space's folder) and its branch; sent when it
    /// changes, `None` when there's no folder.
    Window(Option<WindowContext>),
    /// Events in the protocol's own form (`{"Clicked":{"id":"add"}}`, see
    /// [`crate::Event`]). Ones the host can't read are dropped. `seq` comes back as the answer's `ack`
    /// (round trips; a client may hold pointer moves until the last batch is answered).
    Events {
        #[serde(default)]
        seq: u64,
        events: Vec<serde_json::Value>,
        #[serde(default)]
        mods: Mods,
    },
}

/// A picture ready to show: its file's name in the images folder ([`In::Hello`]'s
/// `images`), and its size in pixels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shown {
    pub file: String,
    pub width: u32,
    pub height: u32,
}

/// Host → client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Message {
    /// The module loaded: its name and description, and how long loading took.
    Ready { name: String, description: String, load_ms: f64 },
    /// What the module shows now. `ack`: the last `seq` this answers; `call_ms`: the time in
    /// the module (all calls this answer took); `got`: when the host read the events it
    /// answers, `sent`: when it wrote this (milliseconds since the epoch).
    /// `images`: each picture's key ([`crate::ImageSrc`]: a path, `handle:<n>`, a bytes
    /// key, or `canvas:<id>` for a big canvas the host drew) → its file and size.
    Frame {
        title: String,
        ui: Node,
        actions: Vec<Offer>,
        ack: u64,
        call_ms: f64,
        got: f64,
        sent: f64,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        images: BTreeMap<String, Shown>,
    },
    /// Give a text field the keyboard.
    Focus { id: String },
    /// Open an http(s) link (asked in answer to the user's own click or key).
    OpenUrl { url: String },
    /// Close the module's pane.
    Close,
    /// Open module `module` with `arg` in a new tab beside this one (in answer to the
    /// user's own click or key): the git graph's diff of a commit.
    Open { module: String, arg: String },
    /// The module can't run (or stopped): why.
    Error { message: String },
}

/// A module's name: what [`In::Hello`] may ask for. Plain names only, never a path.
pub fn module_name_ok(name: &str) -> bool {
    (1..=64).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}
