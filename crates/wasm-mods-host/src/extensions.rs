//! What the permissions screen sees and changes, through the host CLI ([`crate::cli`]):
//! every installed module with the answers for each capability it declares
//! ([`ModuleAccess`]), turning modules off and on, and removing them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use wasm_mods_protocol::{CapabilityAccess, ModuleAccess, PROTOCOL_VERSION};

use crate::engine::is_module;
use crate::permissions::{self, Capability, Decision, Manifest, ModuleId};

/// The `origin` of every module here.
const ORIGIN: &str = "wasm-mods";

/// What a module file says, kept while the file is unchanged (reading and hashing every
/// module on every call would be slow).
#[derive(Clone)]
struct Read {
    hash: String,
    manifest: Option<Manifest>,
}

static READ: Mutex<Option<HashMap<PathBuf, (SystemTime, Read)>>> = Mutex::new(None);

fn read(path: &Path) -> Option<Read> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    let mut cache = READ.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some((t, r)) = cache.get(path)
        && *t == modified
    {
        return Some(r.clone());
    }
    let bytes = std::fs::read(path).ok()?;
    if !is_module(&bytes) {
        return None;
    }
    let r = Read { hash: permissions::hash(&bytes), manifest: permissions::read_manifest(&bytes) };
    cache.insert(path.to_owned(), (modified, r.clone()));
    Some(r)
}

/// Every module in `dir`, by name, with what it may do.
pub fn list(dir: &Path) -> Vec<ModuleAccess> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok()?.strip_suffix(".wasm").map(str::to_owned))
        .filter(|n| crate::wire::module_name_ok(n))
        .collect();
    names.sort();
    permissions::with(|grants| {
        names
            .into_iter()
            .filter_map(|name| {
                let r = read(&dir.join(format!("{name}.wasm")))?;
                let id = ModuleId { name: name.clone() };
                let capabilities = r
                    .manifest
                    .as_ref()
                    .map(|m| m.capabilities.clone())
                    .unwrap_or_default()
                    .iter()
                    .map(|cap| {
                        let available = !matches!(cap, Capability::Unavailable(_));
                        let state = match grants.decide(&id, &r.hash, cap) {
                            Decision::Granted => "granted",
                            Decision::Denied if available => "denied",
                            Decision::Denied => "unavailable",
                            Decision::Ask => "ask",
                        };
                        CapabilityAccess {
                            id: cap.id(),
                            about: cap.about(),
                            state: state.into(),
                            available,
                            changeable: available,
                        }
                    })
                    .collect();
                Some(ModuleAccess {
                    origin: ORIGIN.into(),
                    description: r.manifest.as_ref().map(|m| m.description.clone()).unwrap_or_default(),
                    version: String::new(),
                    hash: r.hash.clone(),
                    has_manifest: r.manifest.is_some(),
                    capabilities,
                    protocol: PROTOCOL_VERSION,
                    enabled: !grants.is_off(&id),
                    can_disable: true,
                    removable: true,
                    name,
                })
            })
            .collect()
    })
}

/// Whether module `name` is turned off.
pub fn is_off(name: &str) -> bool {
    permissions::with(|g| g.is_off(&ModuleId { name: name.into() }))
}

fn module(dir: &Path, name: &str) -> Result<(ModuleId, Read, PathBuf), String> {
    if !crate::wire::module_name_ok(name) {
        return Err(format!("{name:?} isn't a module name"));
    }
    let path = dir.join(format!("{name}.wasm"));
    let r = read(&path).ok_or_else(|| format!("no module {name}"))?;
    Ok((ModuleId { name: name.into() }, r, path))
}

/// A change the permissions screen asks for (see [`crate::cli`]). There's no "allow":
/// a capability is allowed only from the module's own prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// Deny a capability the module declares.
    Deny { module: String, capability: String },
    /// Forget the answer for one: the module asks again.
    Forget { module: String, capability: String },
    /// Turn the module off (it isn't loaded) or back on.
    SetEnabled { module: String, enabled: bool },
    /// Delete its `.wasm` and forget its answers.
    Remove { module: String },
}

/// Carries out `change` on the modules in `dir`.
pub fn apply(dir: &Path, change: &Change) -> Result<(), String> {
    match change {
        Change::Deny { module: name, capability } | Change::Forget { module: name, capability } => {
            let (id, r, _) = module(dir, name)?;
            let cap = Capability::parse(capability);
            let declared = r.manifest.as_ref().is_some_and(|m| m.capabilities.contains(&cap));
            if !declared || matches!(cap, Capability::Unavailable(_)) {
                return Err(format!("{name} doesn't declare {capability}"));
            }
            let deny = matches!(change, Change::Deny { .. });
            permissions::save(|g| if deny { g.set(&id, &r.hash, &cap, false) } else { g.forget(&id, &cap) });
            Ok(())
        }
        Change::SetEnabled { module: name, enabled } => module(dir, name).map(|(id, _, _)| {
            permissions::save(|g| g.set_off(&id, !enabled));
        }),
        Change::Remove { module: name } => module(dir, name).and_then(|(id, _, path)| {
            std::fs::remove_file(&path).map_err(|e| format!("can't remove {}: {e}", path.display()))?;
            permissions::save(|g| g.forget_module(&id));
            Ok(())
        }),
    }
}
