//! The file chooser on macOS: AppleScript's `choose file`, `choose folder` and
//! `choose file name`, run with `osascript`. The title, the suggested name and the
//! extensions go in as the script's arguments, never into the script's text.

use std::process::Command;

use super::portal::Ask;

/// `osascript`'s arguments for `ask`: the script's lines (`-e`), then its arguments
/// (the title first).
pub fn args(title: &str, ask: &Ask) -> Vec<String> {
    let (expr, rest): (&str, Vec<String>) = match ask {
        Ask::File { extensions } if extensions.is_empty() => ("choose file with prompt (item 1 of argv)", vec![]),
        Ask::File { extensions } => {
            ("choose file with prompt (item 1 of argv) of type (items 2 thru -1 of argv)", extensions.clone())
        }
        Ask::Folder => ("choose folder with prompt (item 1 of argv)", vec![]),
        Ask::Save { suggested } if suggested.is_empty() => ("choose file name with prompt (item 1 of argv)", vec![]),
        Ask::Save { suggested } => {
            ("choose file name with prompt (item 1 of argv) default name (item 2 of argv)", vec![suggested.clone()])
        }
    };
    let mut out: Vec<String> = ["on run argv", &format!("POSIX path of ({expr})"), "end run"]
        .iter()
        .flat_map(|line| ["-e".to_owned(), (*line).to_owned()])
        .collect();
    out.push(title.to_owned());
    out.extend(rest);
    out
}

/// A local path as a `file://` URI that [`super::portal::uri_path`] reads back.
pub fn uri_of(path: &str) -> String {
    format!("file://{}", path.replace('%', "%25"))
}

/// What `osascript` said: the chosen path as a URI, `None` when the user cancelled
/// (AppleScript's error -128).
pub fn outcome(ok: bool, stdout: &str, stderr: &str) -> Result<Option<Vec<String>>, String> {
    if !ok {
        if stderr.contains("(-128)") {
            return Ok(None);
        }
        return Err(format!("the file chooser: {}", stderr.trim()));
    }
    let path = stdout.trim_end_matches('\n');
    // A folder comes with a trailing slash.
    let path = if path.len() > 1 { path.trim_end_matches('/') } else { path };
    if !path.starts_with('/') {
        return Err("the file chooser gave no path".into());
    }
    Ok(Some(vec![uri_of(path)]))
}

/// Shows the chooser and waits for the user.
pub fn choose(title: &str, ask: &Ask) -> Result<Option<Vec<String>>, String> {
    let out = Command::new("osascript")
        .args(args(title, ask))
        .output()
        .map_err(|e| format!("the file chooser (osascript): {e}"))?;
    outcome(out.status.success(), &String::from_utf8_lossy(&out.stdout), &String::from_utf8_lossy(&out.stderr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::portal::uri_path;
    use std::path::PathBuf;

    fn script(a: &[String]) -> String {
        a.chunks(2).take(3).map(|c| c[1].clone()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn the_script_takes_everything_as_arguments() {
        let title = "notes: open a file \" & (do shell script \"x\") & \"";
        let a = args(title, &Ask::File { extensions: vec!["md".into(), "txt".into()] });
        assert_eq!(script(&a), "on run argv\nPOSIX path of (choose file with prompt (item 1 of argv) of type (items 2 thru -1 of argv))\nend run");
        assert_eq!(&a[6..], [title, "md", "txt"]);
        let a = args("t", &Ask::File { extensions: vec![] });
        assert_eq!(&a[6..], ["t"]);
        assert!(script(&a).contains("(choose file with prompt (item 1 of argv))"));
        assert!(script(&args("t", &Ask::Folder)).contains("choose folder with prompt"));
        let a = args("t", &Ask::Save { suggested: "out.md".into() });
        assert!(script(&a).contains("default name (item 2 of argv)"));
        assert_eq!(&a[6..], ["t", "out.md"]);
        assert!(!script(&args("t", &Ask::Save { suggested: String::new() })).contains("default name"));
    }

    #[test]
    fn answers_become_uris_that_read_back() {
        let path = "/Users/u/My Notes/100% é.md";
        let got = outcome(true, &format!("{path}\n"), "").unwrap().unwrap();
        assert_eq!(uri_path(&got[0]), Some(PathBuf::from(path)));
        let got = outcome(true, "/Users/u/Docs/\n", "").unwrap().unwrap();
        assert_eq!(uri_path(&got[0]), Some(PathBuf::from("/Users/u/Docs")));
        assert_eq!(outcome(true, "/\n", "").unwrap().unwrap(), vec!["file:///".to_owned()]);
    }

    #[test]
    fn cancel_and_failures() {
        assert_eq!(outcome(false, "", "0:30: execution error: User canceled. (-128)\n"), Ok(None));
        assert!(outcome(false, "", "something else").is_err());
        assert!(outcome(true, "", "").is_err());
    }
}
