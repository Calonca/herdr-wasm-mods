//! The permissions on the command line, for herdr-app's permissions screen (the core
//! `wasm-mods` mod) and people: the list and its changes, through [`crate::extensions`], so
//! permissions.toml stays the one place they're kept and the rules (a capability is
//! allowed only from the module's own prompt) stay here. It gives nothing new: the
//! person running it can edit that file anyway.
//!
//!   wasm-mods-host --permissions                   every module, as JSON
//!   wasm-mods-host --deny    MODULE CAPABILITY
//!   wasm-mods-host --forget  MODULE CAPABILITY     asks again next time
//!   wasm-mods-host --enable|--disable MODULE
//!   wasm-mods-host --remove  MODULE                deletes its .wasm, forgets it
//!
//! A change prints `{"ok": true, "note": …}`; a refusal goes to stderr with exit code 1.

use std::path::Path;

use crate::extensions::{self, Change};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    List,
    Deny { module: String, capability: String },
    Forget { module: String, capability: String },
    Enable { module: String, on: bool },
    Remove { module: String },
}

/// The command in `args` (without the program's name), if one of these flags is there:
/// `None` when it isn't a permissions call, `Some(Err)` when its arguments are missing.
pub fn parse(args: &[String]) -> Option<Result<Command, String>> {
    let at = args.iter().position(|a| matches!(a.as_str(), "--permissions" | "--deny" | "--forget" | "--enable" | "--disable" | "--remove"))?;
    let flag = args[at].as_str();
    let rest: Vec<String> = args[at + 1..].iter().take_while(|a| !a.starts_with("--")).cloned().collect();
    let need = |n: usize| -> Result<(), String> {
        if rest.len() == n { Ok(()) } else { Err(format!("{flag} takes {n} arguments, got {}", rest.len())) }
    };
    Some((|| {
        Ok(match flag {
            "--permissions" => Command::List,
            "--deny" | "--forget" => {
                need(2)?;
                let (module, capability) = (rest[0].clone(), rest[1].clone());
                match flag {
                    "--forget" => Command::Forget { module, capability },
                    _ => Command::Deny { module, capability },
                }
            }
            _ => {
                need(1)?;
                let module = rest[0].clone();
                match flag {
                    "--remove" => Command::Remove { module },
                    _ => Command::Enable { module, on: flag == "--enable" },
                }
            }
        })
    })())
}

/// Carries out `cmd` on the modules in `dir`: what to print.
pub fn run(dir: &Path, cmd: &Command) -> Result<String, String> {
    let done = |note: &str| Ok(serde_json::json!({ "ok": true, "note": note }).to_string());
    match cmd {
        Command::List => serde_json::to_string(&extensions::list(dir)).map_err(|e| e.to_string()),
        Command::Deny { module, capability } => {
            extensions::apply(dir, &Change::Deny { module: module.clone(), capability: capability.clone() })?;
            done("denied")
        }
        Command::Forget { module, capability } => {
            extensions::apply(dir, &Change::Forget { module: module.clone(), capability: capability.clone() })?;
            done("forgotten: it asks again")
        }
        Command::Enable { module, on } => {
            extensions::apply(dir, &Change::SetEnabled { module: module.clone(), enabled: *on })?;
            done(if *on { "turned on" } else { "turned off" })
        }
        Command::Remove { module } => {
            extensions::apply(dir, &Change::Remove { module: module.clone() })?;
            done("removed")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn parses_the_permission_commands_and_refuses_bad_ones() {
        assert_eq!(parse(&args("--port-file x")), None);
        assert_eq!(parse(&args("--permissions")), Some(Ok(Command::List)));
        assert_eq!(
            parse(&args("--modules /m --deny todo fs.read:~/notes")),
            Some(Ok(Command::Deny { module: "todo".into(), capability: "fs.read:~/notes".into() }))
        );
        assert_eq!(
            parse(&args("--forget todo fs.write:~/notes")),
            Some(Ok(Command::Forget { module: "todo".into(), capability: "fs.write:~/notes".into() }))
        );
        assert_eq!(parse(&args("--disable todo")), Some(Ok(Command::Enable { module: "todo".into(), on: false })));
        assert_eq!(parse(&args("--remove hello")), Some(Ok(Command::Remove { module: "hello".into() })));
        // Missing or extra arguments: refused, not guessed.
        assert!(matches!(parse(&args("--deny todo")), Some(Err(_))));
        assert!(matches!(parse(&args("--remove hello frame")), Some(Err(_))));
    }
}
