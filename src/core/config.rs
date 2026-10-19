//! `config.toml`: the app's own settings (app-settings: Config file location).

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::office::config_xml::DeploymentConfig;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InstallState {
    /// Office setup finished successfully at least once in the current prefix.
    pub office_installed: bool,
    /// Product ID of the installed Office, for LicensingNext and display.
    pub product_id: Option<String>,
    /// Hide the "Install for this user" offer.
    pub self_install_dismissed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Mirrors the `runners/active` symlink.
    pub active_runner: Option<String>,
    pub theme: ThemePref,
    /// Opt-in: make Office apps the default handlers for their MIME types.
    pub default_handlers: bool,
    /// DPI Office is told to draw at (display scaling); `None` keeps Wine's 96.
    pub office_dpi: Option<u32>,
    pub deployment: DeploymentConfig,
    pub install: InstallState,
}

#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    /// Set when the file was unreadable and has been moved to `config.toml.bak`.
    pub notice: Option<String>,
}

impl Config {
    /// Load the config. A missing file gives defaults; an unreadable one is renamed to
    /// `config.toml.bak` and defaults are used, with a notice for the UI.
    pub fn load(path: &Path) -> Loaded {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Loaded { config: Config::default(), notice: None };
            }
            Err(e) => return Self::backup(path, &e.to_string()),
        };
        match toml::from_str::<Config>(&text) {
            Ok(config) => Loaded { config, notice: None },
            Err(e) => Self::backup(path, &e.to_string()),
        }
    }

    fn backup(path: &Path, reason: &str) -> Loaded {
        let bak = path.with_extension("toml.bak");
        let notice = match std::fs::rename(path, &bak) {
            Ok(()) => format!(
                "Your settings file could not be read and was reset to defaults. The old file was kept as {}. ({reason})",
                bak.display()
            ),
            Err(e) => format!("Your settings file could not be read and was reset to defaults ({reason}; backup failed: {e})."),
        };
        log::warn!("{notice}");
        Loaded { config: Config::default(), notice: Some(notice) }
    }

    /// Write atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
        write_atomic(path, text.as_bytes())
    }
}

/// The config shared by the UI thread and the worker; every update is saved immediately.
#[derive(Clone)]
pub struct SharedConfig {
    inner: std::sync::Arc<std::sync::Mutex<Config>>,
    path: std::path::PathBuf,
}

impl SharedConfig {
    pub fn new(config: Config, path: std::path::PathBuf) -> Self {
        Self { inner: std::sync::Arc::new(std::sync::Mutex::new(config)), path }
    }

    pub fn get(&self) -> Config {
        self.inner.lock().unwrap().clone()
    }

    /// Change the config and save it.
    pub fn update<R>(&self, f: impl FnOnce(&mut Config) -> R) -> R {
        let mut c = self.inner.lock().unwrap();
        let r = f(&mut c);
        if let Err(e) = c.save(&self.path) {
            log::error!("could not save {}: {e}", self.path.display());
        }
        r
    }
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::office::plans::Plan;

    #[test]
    fn missing_file_gives_defaults_without_notice() {
        let tmp = tempfile::tempdir().unwrap();
        let loaded = Config::load(&tmp.path().join("config.toml"));
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.notice.is_none());
    }

    #[test]
    fn valid_file_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sub/config.toml");
        let mut cfg = Config::default();
        cfg.active_runner = Some("soda-11.0-32-experimental".into());
        cfg.theme = ThemePref::Dark;
        cfg.default_handlers = true;
        cfg.deployment.plan = Plan::Home2024;
        cfg.install.office_installed = true;
        cfg.save(&path).unwrap();
        let loaded = Config::load(&path);
        assert_eq!(loaded.config, cfg);
        assert!(loaded.notice.is_none());
    }

    #[test]
    fn partial_file_fills_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "theme = \"light\"\n[deployment]\nplan = \"pro-plus2024\"\n").unwrap();
        let loaded = Config::load(&path);
        assert_eq!(loaded.config.theme, ThemePref::Light);
        assert_eq!(loaded.config.deployment.plan, Plan::ProPlus2024);
        assert!(loaded.config.deployment.match_os);
    }

    #[test]
    fn corrupt_file_is_backed_up() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "this is = = not toml [").unwrap();
        let loaded = Config::load(&path);
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.notice.is_some());
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("config.toml.bak")).unwrap(),
            "this is = = not toml ["
        );
    }
}
