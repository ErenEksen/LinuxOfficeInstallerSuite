//! Every location the app reads or writes. Wine data lives under `~/.msoffice/`; the
//! app's own settings follow the XDG base directory spec.

use std::io;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

pub const APP_ID: &str = "linux-office-installer-suite";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub home: PathBuf,
    /// `$XDG_CONFIG_HOME/linux-office-installer-suite`
    pub config_dir: PathBuf,
    /// `$XDG_CONFIG_HOME` itself (holds `mimeapps.list`).
    pub xdg_config_home: PathBuf,
    /// `$XDG_DATA_HOME` (normally `~/.local/share`).
    pub xdg_data_home: PathBuf,
    /// `~/.msoffice`
    pub root: PathBuf,
}

impl Paths {
    /// Resolve from the process environment.
    pub fn from_env() -> Self {
        Self::resolve(|k| std::env::var_os(k).map(PathBuf::from))
    }

    /// Resolve with an injectable environment lookup (used by tests).
    pub fn resolve(env: impl Fn(&str) -> Option<PathBuf>) -> Self {
        let home = env("HOME").filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from("/"));
        // Relative XDG values are invalid per the spec and must be ignored.
        let xdg = |key: &str, fallback: &str| {
            env(key).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(fallback))
        };
        let xdg_config_home = xdg("XDG_CONFIG_HOME", ".config");
        let xdg_data_home = xdg("XDG_DATA_HOME", ".local/share");
        Self {
            config_dir: xdg_config_home.join(APP_ID),
            root: home.join(".msoffice"),
            home,
            xdg_config_home,
            xdg_data_home,
        }
    }

    #[cfg(test)]
    pub fn for_home(home: &Path) -> Self {
        let home = home.to_path_buf();
        Self::resolve(move |k| (k == "HOME").then(|| home.clone()))
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }
    pub fn ledger_file(&self) -> PathBuf {
        self.config_dir.join("ledger.toml")
    }
    pub fn instance_lock(&self) -> PathBuf {
        self.config_dir.join("instance.lock")
    }

    pub fn runners(&self) -> PathBuf {
        self.root.join("runners")
    }
    pub fn runner(&self, tag: &str) -> PathBuf {
        self.runners().join(tag)
    }
    pub fn active_runner_link(&self) -> PathBuf {
        self.runners().join("active")
    }
    pub fn prefix(&self) -> PathBuf {
        self.root.join("prefix")
    }
    pub fn prefix_state(&self) -> PathBuf {
        self.prefix().join(".lois-state.toml")
    }
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }
    pub fn bin(&self) -> PathBuf {
        self.root.join("bin")
    }
    pub fn launch_wrapper(&self) -> PathBuf {
        self.bin().join("msoffice-launch")
    }
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn configuration_xml(&self) -> PathBuf {
        self.root.join("Configuration.xml")
    }
    pub fn selfcheck_prefix(&self) -> PathBuf {
        self.cache().join("selfcheck-prefix")
    }

    pub fn applications(&self) -> PathBuf {
        self.xdg_data_home.join("applications")
    }
    pub fn icons_hicolor(&self) -> PathBuf {
        self.xdg_data_home.join("icons/hicolor")
    }
    pub fn mimeapps(&self) -> PathBuf {
        self.xdg_config_home.join("mimeapps.list")
    }
    pub fn local_bin(&self) -> PathBuf {
        self.home.join(".local/bin")
    }
    pub fn installed_binary(&self) -> PathBuf {
        self.local_bin().join(APP_ID)
    }

    /// Create `~/.msoffice/{runners,prefix,cache,bin,logs}`.
    pub fn ensure_layout(&self) -> io::Result<()> {
        for dir in [self.runners(), self.prefix(), self.cache(), self.bin(), self.logs()] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_from_home() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Paths::for_home(tmp.path());
        assert_eq!(p.config_file(), tmp.path().join(".config/linux-office-installer-suite/config.toml"));
        assert_eq!(p.prefix(), tmp.path().join(".msoffice/prefix"));
        assert_eq!(p.launch_wrapper(), tmp.path().join(".msoffice/bin/msoffice-launch"));
        assert_eq!(p.applications(), tmp.path().join(".local/share/applications"));
        assert_eq!(p.icons_hicolor(), tmp.path().join(".local/share/icons/hicolor"));
        assert_eq!(p.installed_binary(), tmp.path().join(".local/bin/linux-office-installer-suite"));
    }

    #[test]
    fn honours_absolute_xdg_and_ignores_relative() {
        let p = Paths::resolve(|k| match k {
            "HOME" => Some("/h".into()),
            "XDG_CONFIG_HOME" => Some("/cfg".into()),
            "XDG_DATA_HOME" => Some("relative/data".into()),
            _ => None,
        });
        assert_eq!(p.config_dir, PathBuf::from("/cfg/linux-office-installer-suite"));
        assert_eq!(p.applications(), PathBuf::from("/h/.local/share/applications"));
    }

    #[test]
    fn ensure_layout_creates_the_five_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Paths::for_home(tmp.path());
        p.ensure_layout().unwrap();
        let mut names: Vec<_> = std::fs::read_dir(&p.root)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["bin", "cache", "logs", "prefix", "runners"]);
    }
}
