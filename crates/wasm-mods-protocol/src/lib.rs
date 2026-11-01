//! What the wasm-mods host and a WASM mod exchange.
//!
//! A WASM mod is a small app compiled to WebAssembly. The host calls it with an
//! [`Input`] (size, theme, events since the last call, host data the mod asked for)
//! and gets back an [`Output`]: a tree of prebuilt components ([`Node`]) that herdr-app
//! draws natively (the `wasm-mods` JavaScript mod turns it into GPUI elements), plus
//! [`Effect`]s for the host to carry out. Mods never draw or touch the system
//! themselves, which keeps them sandboxed.
//!
//! Both sides serialize these types as JSON (simple, debuggable; a binary format can
//! replace it later without changing the types).
//!
//! Colours are `0xRRGGBBAA`.
//!
//! The components are 8 primitives ([`Node`]): `Empty`, `Column`, `Row`, `Text`,
//! `TextField`, `List`, `Canvas` and `Popup`, with shared [`Style`] and [`Layout`]
//! props. The [`ui`] module builds them (`heading`, `button`, `tile`, `list`, …) and is
//! used by every mod.

use serde::{Deserialize, Serialize};

pub mod ui;
pub mod wire;

/// Bumped when these types change incompatibly. A host runs modules built for any
/// protocol from [`PROTOCOL_MIN`] to this one (`wasm-mods-host --version --json` says
/// which), so a newer host keeps running older modules for as long as it can. Protocol 5
/// (host 0.2.0): a list can be the module's page ([`Node::List`]'s `pages`:
/// PageUp/PageDown/Home/End scroll it), which a protocol 4 host would drop. Additions
/// within a protocol are new fields with defaults and new events, which an older SDK of
/// the same protocol skips ([`Input::events`]).
pub const PROTOCOL_VERSION: u32 = 5;

/// The oldest protocol this host still runs modules for.
pub const PROTOCOL_MIN: u32 = 4;

/// Whether the host runs a module built for protocol `version`.
pub fn supported(version: u32) -> bool {
    (PROTOCOL_MIN..=PROTOCOL_VERSION).contains(&version)
}

pub type Color = u32;

mod fs;
mod input;
mod node;
mod output;

pub use fs::*;
pub use input::*;
pub use node::*;
pub use output::*;
