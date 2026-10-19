//! Preparing `~/.msoffice/prefix` (wine-prefix-setup).

pub mod display;
pub mod fonts;
pub mod graphics;
pub mod redist;
pub mod steps;
pub mod web;
pub mod wine;

use std::path::PathBuf;

use reqwest::blocking::Client;

use crate::core::paths::Paths;
use crate::core::proc::{OpLog, Stream, run};
use crate::core::{Cancel, Report};
use steps::{Runner, Step, StepError, StepEvent};
use wine::{RegData, WinVersion, Wine};

/// Everything a setup step needs.
pub struct PrefixCtx<'a> {
    pub paths: &'a Paths,
    pub wine: Wine,
    pub client: &'a Client,
    pub log: &'a OpLog,
    pub cancel: &'a Cancel,
    pub report: &'a mut dyn FnMut(Report),
}

impl PrefixCtx<'_> {
    fn exec(&mut self, cmd: &crate::core::proc::Cmd) -> Result<(), StepError> {
        let report = &mut *self.report;
        self.wine.exec(cmd, Some(self.log), &mut |_: Stream, l: &str| report(Report::Line(l.to_string())), self.cancel)?;
        Ok(())
    }

    fn import(&mut self, entries: &[(String, String, RegData)]) -> Result<(), StepError> {
        self.wine.reg_import(entries, Some(self.log), self.cancel)?;
        Ok(())
    }

    fn set_version(&mut self, v: WinVersion) -> Result<(), StepError> {
        self.import(&wine::windows_version_entries(v))
    }

    fn fetch(&mut self, archive: &redist::Archive, sub: &str) -> Result<PathBuf, StepError> {
        let dir = self.paths.cache().join(sub);
        let report = &mut *self.report;
        let log = self.log;
        let mut meter = crate::core::download::Meter::new();
        archive.fetch(
            self.client,
            &dir,
            &mut |d, t| {
                report(Report::Progress(t.filter(|t| *t > 0).map(|t| d as f32 / t as f32)));
                let u = meter.update(d, t);
                if let Some(live) = u.live {
                    report(Report::Detail(format!("Downloading {}: {live}", archive.file)));
                }
                if let Some(line) = u.log {
                    let line = format!("{}: {line}", archive.file);
                    log.line(&line);
                    report(Report::Line(line));
                }
            },
            self.log,
            self.cancel,
        )
    }

    fn work_dir(&self, name: &str) -> PathBuf {
        self.paths.cache().join(format!("work-{name}"))
    }
}

/// Registry value that keeps Wine's menu builder off inside the prefix even for processes
/// we did not start (the env var covers ours).
fn menubuilder_off() -> Vec<(String, String, RegData)> {
    vec![(r"HKEY_CURRENT_USER\Software\Wine\DllOverrides".into(), "winemenubuilder.exe".into(), RegData::Sz(String::new()))]
}

/// The prefix setup steps in recipe order: create, win7, fonts, redistributables, DLL
/// overrides, graphics, then the web components for Office's sign-in window, and win10.
/// Steps added later run on their own in prefixes set up before them.
pub fn setup_steps<'a>() -> Vec<Step<PrefixCtx<'a>>> {
    let mut v: Vec<Step<PrefixCtx<'a>>> = Vec::new();
    v.push(Step::new("create", "Create the Wine prefix", |c: &mut PrefixCtx| {
        std::fs::create_dir_all(&c.wine.prefix)?;
        (c.report)(Report::Progress(None));
        let boot = c.wine.wine(["wineboot", "-u"]).env("WINEDLLOVERRIDES", "winemenubuilder.exe=d;mscoree=d;mshtml=d");
        c.exec(&boot)?;
        c.exec(&c.wine.wineserver_wait())?;
        if !c.wine.is_initialised() {
            return Err(StepError::msg("wineboot finished but the prefix is incomplete"));
        }
        c.import(&menubuilder_off())
    }));
    v.push(Step::new("win7", "Set Windows version to Windows 7", |c: &mut PrefixCtx| c.set_version(WinVersion::Win7)));
    v.push(Step::new("fonts", "Install fonts", |c: &mut PrefixCtx| {
        let mut entries = Vec::new();
        for pack in fonts::CORE_FONTS {
            let archive = c.fetch(&pack.archive, "fonts")?;
            fonts::install_cab_pack(pack, &archive, &c.wine, &c.work_dir("fonts"), c.log, c.cancel)?;
            entries.extend(fonts::registrations(pack));
        }
        let archive = c.fetch(&fonts::SELAWIK.archive, "fonts")?;
        fonts::install_zip_pack(&fonts::SELAWIK, &archive, &c.wine, c.log)?;
        entries.extend(fonts::registrations(&fonts::SELAWIK));
        entries.extend(fonts::substitutes());
        c.import(&entries)
    }));
    for pkg in redist::PACKAGES {
        v.push(Step::new(pkg.id, format!("Install {}", pkg.title), move |c: &mut PrefixCtx| {
            let archive = c.fetch(pkg.archive, "redist")?;
            (c.report)(Report::Progress(None));
            redist::install_package(pkg, &archive, &c.wine, &c.work_dir(pkg.id), c.log, c.cancel)
        }));
    }
    v.push(Step::new("overrides", "Set DLL overrides", |c: &mut PrefixCtx| {
        let entries: Vec<_> = redist::OVERRIDES
            .iter()
            .map(|(dll, mode)| (r"HKEY_CURRENT_USER\Software\Wine\DllOverrides".to_string(), (*dll).to_string(), RegData::Sz((*mode).into())))
            .collect();
        c.import(&entries)
    }));
    v.push(Step::new("dxvk", "Install DXVK", |c: &mut PrefixCtx| {
        let archive = c.fetch(&graphics::DXVK, "dxvk")?;
        let files = graphics::install_dxvk(&archive, &c.wine)?;
        c.log.line(&format!("installed DXVK: {}", files.join(", ")));
        c.import(&graphics::dxvk_overrides())
    }));
    v.push(Step::new("graphics", "Apply graphics settings", |c: &mut PrefixCtx| {
        c.import(&graphics::settings())?;
        // Exit code 1 = value absent, which is the goal.
        let del = c.wine.reg_delete(graphics::DIRECT2D_KEY, Some("max_version_factory"));
        let report = &mut *c.report;
        let out = run(&del, Some(c.log), &mut |_, l| report(Report::Line(l.into())), c.cancel)?;
        match out.status.code() {
            Some(0) | Some(1) => Ok(()),
            _ => Err(StepError::msg(format!("removing Direct2D max_version_factory failed: {}", out.tail(5)))),
        }
    }));
    v.push(Step::new("gecko", "Install Wine Gecko (HTML rendering)", |c: &mut PrefixCtx| {
        for archive in [&web::GECKO_X64, &web::GECKO_X86] {
            let msi = c.fetch(archive, "gecko")?;
            (c.report)(Report::Progress(None));
            web::install_gecko(&c.wine, &msi, c.log, c.cancel)?;
        }
        if !web::gecko_installed(&c.wine) {
            return Err(StepError::msg("Wine Gecko was installed, but its files are missing from the prefix"));
        }
        Ok(())
    }));
    v.push(Step::new("mshtml", "Register the HTML engine", |c: &mut PrefixCtx| {
        (c.report)(Report::Progress(None));
        web::register_mshtml(&c.wine, c.log, c.cancel)
    }));
    v.push(Step::new("webview2", "Install Microsoft Edge WebView2 Runtime", |c: &mut PrefixCtx| {
        if web::webview2_installed(&c.wine) {
            return Ok(());
        }
        let installer = c.fetch(&web::WEBVIEW2, "webview2")?;
        (c.report)(Report::Progress(None));
        web::install_webview2(&c.wine, &installer, c.log, c.cancel)
    }));
    v.push(Step::new("win10", "Set Windows version to Windows 10", |c: &mut PrefixCtx| {
        c.set_version(WinVersion::Win10)?;
        c.exec(&c.wine.wineserver_wait())
    }));
    v
}

/// Run (or resume) prefix setup. `on_step` gets step events with titles resolved.
pub fn run_setup(ctx: &mut PrefixCtx, on_step: &mut dyn FnMut(StepEvent, &str)) -> Result<(), StepError> {
    ctx.paths.ensure_layout()?;
    let steps = setup_steps();
    let titles: Vec<String> = steps.iter().map(|s| s.title.clone()).collect();
    let runner = Runner::new(ctx.paths.prefix_state());
    let log = ctx.log.clone();
    runner.run(&steps, ctx, &mut |e| {
        let i = match &e {
            StepEvent::Skipped(i) | StepEvent::Started(i) | StepEvent::Done(i) | StepEvent::Failed(i, _) => *i,
        };
        log.line(&format!("step {:?}: {}", e, titles[i]));
        on_step(e, &titles[i]);
    })
}

pub fn setup_complete(paths: &Paths) -> bool {
    Runner::new(paths.prefix_state()).is_complete(&setup_steps())
}

/// Reset prefix: stop its processes, delete it and the Office launchers. Runners and the
/// download cache stay (wine-prefix-setup: Prefix reset).
pub fn reset(paths: &Paths, runner: Option<&std::path::Path>) -> std::io::Result<()> {
    if let Some(r) = runner {
        Wine::new(r, paths.prefix()).kill_all();
    }
    crate::desktop::remove_office_integration(paths)?;
    match std::fs::remove_dir_all(paths.prefix()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::fs::create_dir_all(paths.prefix())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_order_matches_the_recipe() {
        let ids: Vec<&str> = setup_steps().iter().map(|s| s.id).collect();
        assert_eq!(
            ids,
            [
                "create", "win7", "fonts", "win7sp1", "msxml6", "riched20", "msls31", "cmd", "overrides", "dxvk", "graphics", "gecko",
                "mshtml", "webview2", "win10"
            ]
        );
    }

    #[test]
    fn reset_keeps_runners_and_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::for_home(tmp.path());
        paths.ensure_layout().unwrap();
        std::fs::write(paths.prefix().join("system.reg"), "x").unwrap();
        std::fs::create_dir_all(paths.runner("soda-x").join("bin")).unwrap();
        std::fs::write(paths.cache().join("a.exe"), "x").unwrap();
        reset(&paths, None).unwrap();
        assert!(!paths.prefix().join("system.reg").exists());
        assert!(paths.runner("soda-x").exists());
        assert!(paths.cache().join("a.exe").exists());
    }
}

/// Live run against the test system's `~/.msoffice`: `cargo test -- --ignored live_prefix`.
#[cfg(test)]
mod live {
    use super::*;
    use crate::core::download::http_client;

    #[test]
    #[ignore]
    fn live_prefix_full_setup() {
        let paths = Paths::from_env();
        let log = OpLog::create(&paths.logs(), "prefix-setup-test").unwrap();
        let client = http_client();
        let cancel = Cancel::new();
        let mut report = |r: Report| {
            if let Report::Line(l) = r {
                eprintln!("  | {l}");
            }
        };
        let mut ctx = PrefixCtx {
            paths: &paths,
            wine: Wine::new(paths.active_runner_link(), paths.prefix()),
            client: &client,
            log: &log,
            cancel: &cancel,
            report: &mut report,
        };
        let t = std::time::Instant::now();
        run_setup(&mut ctx, &mut |e, title| eprintln!("{e:?} {title} [{:?}]", t.elapsed())).unwrap();
        assert!(setup_complete(&paths));
        eprintln!("log: {}", log.path().display());
    }

    #[test]
    #[ignore]
    fn live_prefix_reset_keeps_runners_and_cache() {
        let paths = Paths::from_env();
        let cache_before = std::fs::read_dir(paths.cache().join("redist")).unwrap().count();
        let runners_before = crate::runner::active::installed(&paths);
        assert!(paths.prefix().join("system.reg").exists(), "needs a built prefix");
        reset(&paths, Some(&paths.active_runner_link())).unwrap();
        assert!(!paths.prefix().join("system.reg").exists());
        assert!(!paths.prefix_state().exists());
        assert_eq!(crate::runner::active::installed(&paths), runners_before);
        assert_eq!(std::fs::read_dir(paths.cache().join("redist")).unwrap().count(), cache_before);
        eprintln!("kept runners {runners_before:?} and {cache_before} cached redistributables");
    }
}
