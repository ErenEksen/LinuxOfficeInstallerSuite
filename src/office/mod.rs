pub mod config_xml;
pub mod detect;
pub mod odt;
pub mod pipeline;
pub mod plans;
pub mod postinstall;
pub mod progress;
pub mod source;

use crate::core::config::SharedConfig;
use crate::core::paths::Paths;

/// Uninstall Office: remove every recorded launcher, icon and default handler, then
/// delete the prefix when `delete_prefix` is set (office-installation: Uninstall Office).
pub fn uninstall_office(paths: &Paths, config: &SharedConfig, delete_prefix: bool) -> std::io::Result<()> {
    crate::desktop::remove_office_integration(paths)?;
    if delete_prefix {
        let runner = paths.active_runner_link();
        crate::prefix::reset(paths, runner.exists().then_some(runner.as_path()))?;
        config.update(|c| {
            c.install.office_installed = false;
            c.install.product_id = None;
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Config;
    use crate::core::ledger::{Category, Ledger};

    fn fake_install(paths: &Paths) -> std::path::PathBuf {
        paths.ensure_layout().unwrap();
        let o16 = paths.prefix().join("drive_c/Program Files/Microsoft Office/root/Office16");
        std::fs::create_dir_all(&o16).unwrap();
        std::fs::write(o16.join("WINWORD.EXE"), "MZ").unwrap();
        crate::desktop::install_office_integration(paths, &[detect::App::Word], false, &crate::core::Cancel::new())
            .unwrap();
        // A file of the user's own that must survive.
        let own = paths.applications().join("firefox.desktop");
        std::fs::write(&own, "[Desktop Entry]\n").unwrap();
        own
    }

    #[test]
    fn uninstall_keep_prefix_removes_only_ledger_files() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::for_home(tmp.path());
        let own = fake_install(&paths);
        let cfg = SharedConfig::new(Config::default(), paths.config_file());
        assert!(paths.applications().join("msoffice-word.desktop").exists());
        uninstall_office(&paths, &cfg, false).unwrap();
        assert!(!paths.applications().join("msoffice-word.desktop").exists());
        assert!(own.exists());
        assert!(paths.prefix().join("drive_c/Program Files/Microsoft Office/root/Office16/WINWORD.EXE").exists());
        assert_eq!(Ledger::load(&paths.ledger_file()).files(Category::Office).count(), 0);
        let icons_left: Vec<_> = walk(&paths.icons_hicolor());
        assert!(icons_left.is_empty(), "{icons_left:?}");
    }

    #[test]
    fn uninstall_delete_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::for_home(tmp.path());
        let own = fake_install(&paths);
        let cfg = SharedConfig::new(Config::default(), paths.config_file());
        cfg.update(|c| c.install.office_installed = true);
        uninstall_office(&paths, &cfg, true).unwrap();
        assert!(own.exists());
        assert!(!paths.prefix().join("drive_c").exists());
        assert!(paths.launch_wrapper().exists(), "the wrapper lives in ~/.msoffice/bin");
        assert!(!cfg.get().install.office_installed);
    }

    fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() { out.extend(walk(&p)) } else { out.push(p) }
        }
        out
    }
}
