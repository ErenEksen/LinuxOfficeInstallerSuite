//! The active runner: `runners/active` symlink, mirrored in config.toml (design D6,
//! wine-runner-management: Active runner selection, Runner removal).

use std::io;
use std::path::Path;

use crate::core::config::Config;
use crate::core::paths::Paths;

#[derive(Debug, thiserror::Error)]
pub enum ActiveError {
    #[error("{0} is the active runner and cannot be removed. Set another runner as active first.")]
    IsActive(String),
    #[error("{0} is not installed")]
    NotInstalled(String),
    #[error("{0}")]
    Io(#[from] io::Error),
}

pub const ACTIVE_LINK: &str = "active";

fn is_runner_dir(dir: &Path) -> bool {
    dir.join("bin/wine").exists()
}

/// Installed runner tags, sorted newest-looking first (by name, descending).
pub fn installed(paths: &Paths) -> Vec<String> {
    let mut tags: Vec<String> = std::fs::read_dir(paths.runners())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n != ACTIVE_LINK && !n.starts_with('.'))
        .filter(|n| is_runner_dir(&paths.runner(n)))
        .collect();
    tags.sort_by(|a, b| natural_cmp(b, a));
    tags
}

/// Compare with digit runs as numbers, so `soda-11.0-32` sorts after `soda-11.0-4`.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let split = |s: &str| {
        let mut parts: Vec<(bool, String)> = Vec::new();
        for c in s.chars() {
            let d = c.is_ascii_digit();
            match parts.last_mut() {
                Some((pd, p)) if *pd == d => p.push(c),
                _ => parts.push((d, c.to_string())),
            }
        }
        parts
    };
    let (pa, pb) = (split(a), split(b));
    for (x, y) in pa.iter().zip(&pb) {
        let ord = match (x.0, y.0) {
            (true, true) => x.1.parse::<u64>().unwrap_or(0).cmp(&y.1.parse::<u64>().unwrap_or(0)),
            _ => x.1.cmp(&y.1),
        };
        if ord.is_ne() {
            return ord;
        }
    }
    pa.len().cmp(&pb.len())
}

/// The tag the `active` symlink points to, if it points at an installed runner.
pub fn active(paths: &Paths) -> Option<String> {
    let target = std::fs::read_link(paths.active_runner_link()).ok()?;
    let tag = target.file_name()?.to_str()?.to_string();
    is_runner_dir(&paths.runner(&tag)).then_some(tag)
}

/// Point `runners/active` at `tag` (atomically) and record it in the config.
pub fn set_active(paths: &Paths, config: &mut Config, tag: &str) -> Result<(), ActiveError> {
    if !is_runner_dir(&paths.runner(tag)) {
        return Err(ActiveError::NotInstalled(tag.into()));
    }
    let link = paths.active_runner_link();
    let tmp = paths.runners().join(".active.tmp");
    let _ = std::fs::remove_file(&tmp);
    // Relative target: the tree stays valid if ~/.msoffice is moved or HOME differs.
    std::os::unix::fs::symlink(tag, &tmp)?;
    std::fs::rename(&tmp, &link)?;
    config.active_runner = Some(tag.into());
    Ok(())
}

/// Delete an installed runner. The active one is refused.
pub fn remove(paths: &Paths, tag: &str) -> Result<(), ActiveError> {
    if active(paths).as_deref() == Some(tag) {
        return Err(ActiveError::IsActive(tag.into()));
    }
    let dir = paths.runner(tag);
    if !dir.exists() || tag == ACTIVE_LINK || tag.contains('/') || tag.starts_with('.') {
        return Err(ActiveError::NotInstalled(tag.into()));
    }
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

/// Make the symlink and config agree. The symlink wins; if it is missing, the config's
/// choice is restored when that runner is still installed. Returns the active tag.
pub fn reconcile(paths: &Paths, config: &mut Config) -> Option<String> {
    if let Some(tag) = active(paths) {
        config.active_runner = Some(tag.clone());
        return Some(tag);
    }
    let wanted = config.active_runner.clone().filter(|t| is_runner_dir(&paths.runner(t)));
    match wanted {
        Some(tag) => set_active(paths, config, &tag).ok().map(|_| tag),
        None => {
            config.active_runner = None;
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_runner(paths: &Paths, tag: &str) {
        let bin = paths.runner(tag).join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("wine"), "#!/bin/sh\n").unwrap();
    }

    #[test]
    fn switch_and_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::for_home(tmp.path());
        let mut cfg = Config::default();
        fake_runner(&paths, "soda-11.0-32-experimental");
        fake_runner(&paths, "soda-11.0-4-experimental");
        std::fs::create_dir_all(paths.runners().join(".installing-x")).unwrap();
        assert_eq!(installed(&paths), ["soda-11.0-32-experimental", "soda-11.0-4-experimental"]);
        assert_eq!(active(&paths), None);

        set_active(&paths, &mut cfg, "soda-11.0-32-experimental").unwrap();
        assert_eq!(active(&paths).as_deref(), Some("soda-11.0-32-experimental"));
        assert_eq!(cfg.active_runner.as_deref(), Some("soda-11.0-32-experimental"));
        assert!(paths.active_runner_link().join("bin/wine").exists(), "link resolves");
        assert!(installed(&paths).iter().all(|t| t != "active"));

        assert!(matches!(remove(&paths, "soda-11.0-32-experimental"), Err(ActiveError::IsActive(_))));
        set_active(&paths, &mut cfg, "soda-11.0-4-experimental").unwrap();
        remove(&paths, "soda-11.0-32-experimental").unwrap();
        assert_eq!(installed(&paths), ["soda-11.0-4-experimental"]);
        assert!(matches!(set_active(&paths, &mut cfg, "nope"), Err(ActiveError::NotInstalled(_))));
        assert!(matches!(remove(&paths, "active"), Err(ActiveError::NotInstalled(_)) | Err(ActiveError::IsActive(_))));
    }

    #[test]
    fn reconcile_restores_missing_link_from_config() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::for_home(tmp.path());
        fake_runner(&paths, "a");
        let mut cfg = Config { active_runner: Some("a".into()), ..Config::default() };
        assert_eq!(reconcile(&paths, &mut cfg).as_deref(), Some("a"));
        assert_eq!(active(&paths).as_deref(), Some("a"));
        let mut cfg = Config { active_runner: Some("gone".into()), ..Config::default() };
        std::fs::remove_file(paths.active_runner_link()).unwrap();
        assert_eq!(reconcile(&paths, &mut cfg), None);
        assert_eq!(cfg.active_runner, None);
    }
}
