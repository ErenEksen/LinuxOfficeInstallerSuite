//! Optional default handlers (desktop-integration: Optional default handlers).
//!
//! Previous defaults are queried with `xdg-mime query default` and stored in the ledger
//! before anything changes; removal puts them back. When `xdg-mime` is missing, the user's
//! `mimeapps.list` is edited directly.

use std::io;
use std::path::Path;

use crate::core::config::write_atomic;
use crate::core::ledger::Ledger;
use crate::core::proc::{Cmd, capture, which};

/// Current default desktop id for `mime`.
pub fn query_default(mimeapps: &Path, mime: &str) -> Option<String> {
    if which("xdg-mime").is_some() {
        let (_, out) = capture(&Cmd::new("xdg-mime").args(["query", "default", mime])).ok()?;
        let id = out.trim();
        return (!id.is_empty()).then(|| id.to_string());
    }
    read_default(&std::fs::read_to_string(mimeapps).unwrap_or_default(), mime)
}

fn set_default(mimeapps: &Path, desktop_id: &str, mime: &str) -> io::Result<()> {
    if which("xdg-mime").is_some() {
        let (status, _) =
            capture(&Cmd::new("xdg-mime").args(["default", desktop_id, mime])).map_err(io::Error::other)?;
        if status.success() {
            return Ok(());
        }
    }
    let text = std::fs::read_to_string(mimeapps).unwrap_or_default();
    write_atomic(mimeapps, edit_default(&text, mime, Some(desktop_id)).as_bytes())
}

/// Remove our default for `mime` (when there was none before).
fn unset_default(mimeapps: &Path, desktop_id: &str, mime: &str) -> io::Result<()> {
    let Ok(text) = std::fs::read_to_string(mimeapps) else { return Ok(()) };
    if read_default(&text, mime).as_deref() == Some(desktop_id) {
        write_atomic(mimeapps, edit_default(&text, mime, None).as_bytes())?;
    }
    Ok(())
}

/// First entry of `mime=` in `[Default Applications]`.
pub fn read_default(text: &str, mime: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_section = l == "[Default Applications]";
            continue;
        }
        if in_section
            && let Some((k, v)) = l.split_once('=')
            && k.trim() == mime
        {
            return v.split(';').map(str::trim).find(|s| !s.is_empty()).map(String::from);
        }
    }
    None
}

/// Set (`Some`) or remove (`None`) `mime=` in `[Default Applications]`, keeping everything else.
pub fn edit_default(text: &str, mime: &str, id: Option<&str>) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_section = false;
    let mut seen_section = false;
    let mut done = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            if in_section && !done {
                if let Some(id) = id {
                    out.push(format!("{mime}={id};"));
                }
                done = true;
            }
            in_section = l == "[Default Applications]";
            seen_section |= in_section;
        } else if in_section && l.split_once('=').map(|(k, _)| k.trim() == mime).unwrap_or(false) {
            if let Some(id) = id {
                out.push(format!("{mime}={id};"));
            }
            done = true;
            continue;
        }
        out.push(line.to_string());
    }
    if !done && let Some(id) = id {
        if !seen_section {
            if !out.is_empty() && !out.last().unwrap().is_empty() {
                out.push(String::new());
            }
            out.push("[Default Applications]".into());
        }
        out.push(format!("{mime}={id};"));
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// Drop `ids` from every association list (any section); keys left empty are removed.
pub fn strip_ids(text: &str, ids: &[&str]) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if !l.starts_with('[')
            && let Some((k, v)) = l.split_once('=')
        {
            let all: Vec<&str> = v.split(';').map(str::trim).filter(|s| !s.is_empty()).collect();
            let kept: Vec<&str> = all.iter().copied().filter(|s| !ids.contains(s)).collect();
            if kept.len() != all.len() {
                if !kept.is_empty() {
                    out.push(format!("{}={};", k.trim(), kept.join(";")));
                }
                continue;
            }
        }
        out.push(line.to_string());
    }
    let mut s = out.join("\n");
    if text.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Make `desktop_id` the default for each of `mimes`, recording the previous defaults.
pub fn apply_defaults(mimeapps: &Path, ledger: &mut Ledger, desktop_id: &str, mimes: &[&str]) -> io::Result<()> {
    for mime in mimes {
        let previous = query_default(mimeapps, mime).filter(|p| p != desktop_id);
        ledger.record_mime(mime, previous, desktop_id)?;
        set_default(mimeapps, desktop_id, mime)?;
    }
    Ok(())
}

/// Restore every default recorded in the ledger, where ours is still the default.
pub fn restore_defaults(mimeapps: &Path, ledger: &mut Ledger) -> io::Result<()> {
    for entry in ledger.mime_defaults().to_vec() {
        let current = query_default(mimeapps, &entry.mime);
        if current.as_deref() != Some(entry.ours.as_str()) {
            continue; // The user chose something else since; leave it.
        }
        match &entry.previous {
            Some(prev) => set_default(mimeapps, prev, &entry.mime)?,
            None => unset_default(mimeapps, &entry.ours, &entry.mime)?,
        }
    }
    ledger.take_mime_defaults()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_mimeapps_list() {
        let text = "[Added Associations]\ntext/rtf=foo.desktop;\n\n[Default Applications]\ntext/plain=gedit.desktop;\ntext/rtf=libreoffice-writer.desktop;\n";
        assert_eq!(read_default(text, "text/rtf").as_deref(), Some("libreoffice-writer.desktop"));
        let set = edit_default(text, "text/rtf", Some("msoffice-word.desktop"));
        assert_eq!(read_default(&set, "text/rtf").as_deref(), Some("msoffice-word.desktop"));
        assert!(set.contains("[Added Associations]\ntext/rtf=foo.desktop;"));
        assert!(set.contains("text/plain=gedit.desktop;"));
        let removed = edit_default(&set, "text/rtf", None);
        assert_eq!(read_default(&removed, "text/rtf"), None);
        assert!(removed.contains("text/plain=gedit.desktop;"));

        let added = edit_default(text, "application/msword", Some("msoffice-word.desktop"));
        assert_eq!(read_default(&added, "application/msword").as_deref(), Some("msoffice-word.desktop"));
        let fresh = edit_default("", "application/msword", Some("w.desktop"));
        assert_eq!(fresh, "[Default Applications]\napplication/msword=w.desktop;\n");
    }
}
