//! The one-click install pipeline (office-installation: One-click install pipeline).
//!
//! Stages run in order on the worker thread; each reports its status. A failure stops the
//! pipeline with the stage's error, exit code and log; later stages stay pending. Cancel
//! stops the current operation and every Wine process of the prefix; finished stages and
//! resumable prefix steps are kept.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use reqwest::blocking::Client;

use super::config_xml::{self, DeploymentConfig, InstallSource, LOG_DIR_IN_DRIVE_C};
use super::detect::{self, App};
use super::odt;
use super::postinstall;
use super::progress::LogFollower;
use super::source::{self, LocalSource, SourceError};
use crate::core::config::SharedConfig;
use crate::core::paths::Paths;
use crate::core::proc::{OpLog, ProcError, Stream, run};
use crate::core::{Cancel, Report};
use crate::deps;
use crate::desktop;
use crate::prefix::steps::{StepError, StepEvent};
use crate::prefix::wine::Wine;
use crate::prefix::{self, PrefixCtx};
use crate::runner;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Deps,
    Runner,
    SelfCheck,
    Prefix,
    Odt,
    Source,
    Service,
    Setup,
    PostFixes,
    Detect,
    Launchers,
}

impl Stage {
    pub const ALL: [Stage; 11] = [
        Stage::Deps,
        Stage::Runner,
        Stage::SelfCheck,
        Stage::Prefix,
        Stage::Odt,
        Stage::Source,
        Stage::Service,
        Stage::Setup,
        Stage::PostFixes,
        Stage::Detect,
        Stage::Launchers,
    ];

    /// Whether the stage runs when repairing an existing installation.
    pub fn runs_in_repair(self) -> bool {
        matches!(self, Stage::Prefix | Stage::PostFixes | Stage::Detect | Stage::Launchers)
    }

    pub fn title(self) -> &'static str {
        match self {
            Stage::Deps => "Check system dependencies",
            Stage::Runner => "Install Wine runner",
            Stage::SelfCheck => "Check that Wine works",
            Stage::Prefix => "Prepare the Wine prefix",
            Stage::Odt => "Download the Office Deployment Tool",
            Stage::Source => "Download Office files",
            Stage::Service => "Register the Click-to-Run service",
            Stage::Setup => "Install Office",
            Stage::PostFixes => "Apply post-install fixes",
            Stage::Detect => "Detect installed apps",
            Stage::Launchers => "Create launchers",
        }
    }

    pub fn index(self) -> usize {
        Stage::ALL.iter().position(|s| *s == self).unwrap()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Success { apps: Vec<App>, generic_icons: Vec<App>, log: PathBuf },
    Failed { stage: Stage, message: String, exit_code: Option<i32>, log: PathBuf, setup_log: Option<PathBuf> },
    Cancelled { stage: Stage, log: PathBuf },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Stage(Stage, Status),
    /// Fraction of the running stage, `None` = indeterminate.
    Progress(Option<f32>),
    /// Sub-step of the running stage (e.g. a prefix setup step).
    Detail(String),
    Line(String),
}

#[derive(Clone, Debug)]
pub struct Options {
    pub deployment: DeploymentConfig,
    /// The user confirmed installing missing system packages.
    pub install_deps: bool,
    pub default_handlers: bool,
    /// Display scaling to set in the prefix, if the user chose one.
    pub office_dpi: Option<u32>,
    /// Bring an existing installation up to date: only the prefix steps it is missing, the
    /// post-install fixes and the desktop integration run; Office itself is not reinstalled.
    pub repair: bool,
}

struct Fail {
    message: String,
    exit_code: Option<i32>,
    setup_log: Option<PathBuf>,
    cancelled: bool,
}

impl Fail {
    fn msg(m: impl std::fmt::Display) -> Self {
        Fail { message: m.to_string(), exit_code: None, setup_log: None, cancelled: false }
    }
    fn cancelled() -> Self {
        Fail { message: "cancelled".into(), exit_code: None, setup_log: None, cancelled: true }
    }
}

impl From<StepError> for Fail {
    fn from(e: StepError) -> Self {
        match e {
            StepError::Cancelled => Fail::cancelled(),
            StepError::Failed(m) => Fail::msg(m),
        }
    }
}

impl From<ProcError> for Fail {
    fn from(e: ProcError) -> Self {
        match e {
            ProcError::Cancelled => Fail::cancelled(),
            ProcError::Failed { code, .. } => Fail { exit_code: code, ..Fail::msg(e.to_string()) },
            other => Fail::msg(other),
        }
    }
}

impl From<std::io::Error> for Fail {
    fn from(e: std::io::Error) -> Self {
        Fail::msg(e)
    }
}

struct Ctx<'a> {
    paths: &'a Paths,
    config: &'a SharedConfig,
    client: Client,
    log: OpLog,
    cancel: &'a Cancel,
    emit: &'a mut dyn FnMut(Event),
    opts: &'a Options,
    /// The pre-downloaded Office files, when that worked (Stage::Source).
    source: Option<LocalSource>,
}

impl Ctx<'_> {
    fn wine(&self) -> Wine {
        Wine::new(self.paths.active_runner_link(), self.paths.prefix())
    }

    fn check_cancel(&self) -> Result<(), Fail> {
        if self.cancel.is_cancelled() { Err(Fail::cancelled()) } else { Ok(()) }
    }
}

/// Run the whole pipeline. Blocking; call it from the worker thread.
pub fn run_install(
    paths: &Paths,
    config: &SharedConfig,
    opts: &Options,
    cancel: &Cancel,
    emit: &mut dyn FnMut(Event),
) -> Outcome {
    let log = match OpLog::create(&paths.logs(), "install") {
        Ok(l) => l,
        Err(e) => {
            return Outcome::Failed {
                stage: Stage::Deps,
                message: format!("could not create a log file in {}: {e}", paths.logs().display()),
                exit_code: None,
                log: paths.logs(),
                setup_log: None,
            };
        }
    };
    log.line(&format!(
        "Linux Office Installer Suite {} {}",
        env!("CARGO_PKG_VERSION"),
        if opts.repair { "repair" } else { "install" }
    ));
    log.line(&format!("configuration: {:?}", opts.deployment));
    for s in Stage::ALL {
        emit(Event::Stage(s, Status::Pending));
    }
    let mut ctx = Ctx {
        paths,
        config,
        client: crate::core::download::http_client(),
        log: log.clone(),
        cancel,
        emit,
        opts,
        source: None,
    };

    // Refuse mixed architectures before touching anything.
    if let Err(m) = config_xml::check_architecture(&opts.deployment, config_xml::installed_edition_in(&paths.prefix())) {
        (ctx.emit)(Event::Stage(Stage::Deps, Status::Failed));
        log.line(&m);
        return Outcome::Failed { stage: Stage::Deps, message: m, exit_code: None, log: log.path().into(), setup_log: None };
    }

    let mut apps = Vec::new();
    let mut generic_icons = Vec::new();
    for stage in Stage::ALL {
        if opts.repair && !stage.runs_in_repair() {
            (ctx.emit)(Event::Stage(stage, Status::Done));
            continue;
        }
        (ctx.emit)(Event::Stage(stage, Status::Running));
        (ctx.emit)(Event::Progress(None));
        log.line(&format!("== {}", stage.title()));
        let result = ctx.check_cancel().and_then(|_| match stage {
            Stage::Deps => stage_deps(&mut ctx),
            Stage::Runner => stage_runner(&mut ctx),
            Stage::SelfCheck => stage_selfcheck(&mut ctx),
            Stage::Prefix => stage_prefix(&mut ctx),
            Stage::Odt => stage_odt(&mut ctx),
            Stage::Source => stage_source(&mut ctx),
            Stage::Service => stage_service(&mut ctx),
            Stage::Setup => stage_setup(&mut ctx),
            Stage::PostFixes => {
                postinstall::apply(&ctx.wine(), opts.deployment.plan.product_id(), &log, cancel).map_err(Fail::from).and_then(|()| {
                    match opts.office_dpi {
                        Some(dpi) => ctx
                            .wine()
                            .reg_import(&crate::prefix::display::dpi_entries(dpi), Some(&log), cancel)
                            .map_err(|e| Fail::msg(format!("setting the display scaling failed: {e}"))),
                        None => Ok(()),
                    }
                })
            }
            Stage::Detect => {
                apps = detect::detect(&paths.prefix());
                log.line(&format!("detected: {apps:?}"));
                if apps.is_empty() {
                    Err(Fail::msg("Office setup finished, but no Office apps were found in the prefix"))
                } else {
                    Ok(())
                }
            }
            Stage::Launchers => {
                let r = desktop::install_office_integration(paths, &apps, opts.default_handlers, cancel)?;
                generic_icons = r.generic_icons;
                Ok(())
            }
        });
        match result {
            Ok(()) => {
                (ctx.emit)(Event::Progress(Some(1.0)));
                (ctx.emit)(Event::Stage(stage, Status::Done));
            }
            Err(f) => {
                if stage.index() >= Stage::SelfCheck.index() && ctx.paths.active_runner_link().exists() {
                    log.line("stopping all Wine processes of the prefix");
                    ctx.wine().kill_all();
                }
                if f.cancelled {
                    log.line(&format!("cancelled during: {}", stage.title()));
                    (ctx.emit)(Event::Stage(stage, Status::Cancelled));
                    return Outcome::Cancelled { stage, log: log.path().into() };
                }
                log.line(&format!("FAILED: {}", f.message));
                (ctx.emit)(Event::Stage(stage, Status::Failed));
                return Outcome::Failed {
                    stage,
                    message: f.message,
                    exit_code: f.exit_code,
                    log: log.path().into(),
                    setup_log: f.setup_log,
                };
            }
        }
    }
    // Stop the Click-to-Run service and helpers so the first launch starts clean.
    ctx.wine().kill_all();
    config.update(|c| {
        c.install.office_installed = true;
        c.install.product_id = Some(opts.deployment.plan.product_id().into());
        c.deployment = opts.deployment.clone();
    });
    log.line("finished");
    Outcome::Success { apps, generic_icons, log: log.path().into() }
}

fn stage_deps(ctx: &mut Ctx) -> Result<(), Fail> {
    let report = deps::check();
    ctx.log.line(&format!("{} ({:?}); missing: {:?}", report.os.pretty_name, report.family(), report.missing_names()));
    if !report.optional_missing.is_empty() {
        let names: Vec<&str> = report.optional_missing.iter().map(|m| m.name.as_str()).collect();
        ctx.log.line(&format!("optional, not needed for Office: {}", names.join(", ")));
    }
    if let Some(u) = report.undetected {
        return Err(Fail::msg(format!("{}.\n\n{}", u.title(), u.message())));
    }
    if report.ready() {
        return Ok(());
    }
    let names = report.missing_names().join(", ");
    if let deps::install::RepoStatus::Disabled { instructions } = &report.repo {
        return Err(Fail::msg(format!("Missing: {names}.\n\n{instructions}")));
    }
    if !report.can_auto_install() {
        return Err(Fail::msg(format!(
            "Missing: {names}. Install these with your package manager, then start the installation again."
        )));
    }
    if !ctx.opts.install_deps {
        return Err(Fail::msg(format!("Missing: {names}. Use Install packages on the Install page first.")));
    }
    let pkgs = report.packages();
    (ctx.emit)(Event::Detail(format!("Installing {} packages", pkgs.len())));
    let emit = &mut *ctx.emit;
    deps::install::run_install(report.family(), &pkgs, &ctx.log, &mut |_, l| emit(Event::Line(l.into())), ctx.cancel)
        .map_err(|e| match e {
            deps::install::InstallError::Proc(ProcError::Cancelled) => Fail::cancelled(),
            other => Fail::msg(other),
        })?;
    let after = deps::check();
    if after.ready() {
        Ok(())
    } else {
        Err(Fail::msg(format!("Still missing after installing packages: {}", after.missing_names().join(", "))))
    }
}

fn stage_runner(ctx: &mut Ctx) -> Result<(), Fail> {
    let mut cfg = ctx.config.get();
    let emit = &mut *ctx.emit;
    let log = ctx.log.clone();
    let mut meter = crate::core::download::Meter::new();
    let res = runner::ensure_runner(
        &ctx.client,
        ctx.paths,
        &mut cfg,
        &mut |phase, d, t| {
            let f = t.filter(|t| *t > 0).map(|t| d as f32 / t as f32);
            let f = match phase {
                runner::install::Phase::Download => f.map(|f| f * 0.8),
                runner::install::Phase::Extract => f.map(|f| 0.8 + f * 0.2),
            };
            emit(Event::Progress(f));
            if phase == runner::install::Phase::Download {
                let u = meter.update(d, t);
                if let Some(live) = u.live {
                    emit(Event::Detail(format!("Downloading the runner: {live}")));
                }
                if let Some(line) = u.log {
                    let line = format!("runner download: {line}");
                    log.line(&line);
                    emit(Event::Line(line));
                }
            } else if d == 0 {
                emit(Event::Detail("Extracting the runner".into()));
            }
        },
        Some(&ctx.log),
        ctx.cancel,
    );
    let active = cfg.active_runner.clone();
    ctx.config.update(|c| c.active_runner = active);
    match res {
        Ok(_) => Ok(()),
        Err(runner::install::RunnerError::Cancelled) => Err(Fail::cancelled()),
        Err(e) => Err(Fail::msg(e)),
    }
}

fn stage_selfcheck(ctx: &mut Ctx) -> Result<(), Fail> {
    let emit = &mut *ctx.emit;
    let version = deps::selfcheck::run_selfcheck(
        &ctx.paths.active_runner_link(),
        &ctx.paths.selfcheck_prefix(),
        &[],
        Some(&ctx.log),
        &mut |_, l| emit(Event::Line(l.into())),
        ctx.cancel,
    )
    .map_err(|e| match e {
        deps::selfcheck::SelfCheckError::Cancelled => Fail::cancelled(),
        other => Fail::msg(other),
    })?;
    ctx.log.line(&format!("self-check passed: {version}"));
    (ctx.emit)(Event::Detail(version));
    Ok(())
}

fn stage_prefix(ctx: &mut Ctx) -> Result<(), Fail> {
    let total = prefix::setup_steps().len();
    let emit = &mut *ctx.emit;
    // Both closures below need the sink; route them through a channel-free shared cell.
    let sink = std::cell::RefCell::new(emit);
    let step_title = std::cell::RefCell::new(String::new());
    let mut report = |r: Report| match r {
        Report::Progress(p) => (sink.borrow_mut())(Event::Progress(p)),
        Report::Line(l) => (sink.borrow_mut())(Event::Line(l)),
        Report::Detail(d) => (sink.borrow_mut())(Event::Detail(format!("{} · {d}", step_title.borrow()))),
    };
    let mut pctx = PrefixCtx {
        paths: ctx.paths,
        wine: Wine::new(ctx.paths.active_runner_link(), ctx.paths.prefix()),
        client: &ctx.client,
        log: &ctx.log,
        cancel: ctx.cancel,
        report: &mut report,
    };
    let mut on_step = |e: StepEvent, title: &str| {
        if let StepEvent::Started(i) = e {
            *step_title.borrow_mut() = format!("Step {} of {total}: {title}", i + 1);
            (sink.borrow_mut())(Event::Detail(step_title.borrow().clone()));
        }
    };
    prefix::run_setup(&mut pctx, &mut on_step).map_err(Fail::from)
}

fn stage_odt(ctx: &mut Ctx) -> Result<(), Fail> {
    let emit = &mut *ctx.emit;
    let mut meter = crate::core::download::Meter::new();
    let exe = odt::fetch_odt(
        &ctx.client,
        odt::ODT_URL,
        &odt::cache_file(ctx.paths),
        &mut |d, t| {
            emit(Event::Progress(t.filter(|t| *t > 0).map(|t| d as f32 / t as f32)));
            if let Some(live) = meter.update(d, t).live {
                emit(Event::Detail(format!("Downloading setup.exe: {live}")));
            }
        },
        Some(&ctx.log),
        ctx.cancel,
    )
    .map_err(|e| match e {
        odt::OdtError::Cancelled => Fail::cancelled(),
        other => Fail::msg(other),
    })?;
    odt::stage_in_prefix(&exe, &ctx.wine())?;
    Ok(())
}

/// Download the Office files natively (office-installation: Local Office source). A CDN that
/// does not look as expected is not fatal: setup then downloads by itself, only slower.
fn stage_source(ctx: &mut Ctx) -> Result<(), Fail> {
    let emit = &mut *ctx.emit;
    let mut report = |r: Report| match r {
        Report::Progress(p) => emit(Event::Progress(p)),
        Report::Line(l) => emit(Event::Line(l)),
        Report::Detail(d) => emit(Event::Detail(d)),
    };
    let res = source::prepare(&ctx.client, ctx.paths, &ctx.opts.deployment, &ctx.log, &mut report, ctx.cancel)
        .and_then(|s| source::ensure_z_drive(&ctx.paths.prefix()).map(|_| s).map_err(SourceError::from));
    match res {
        Ok(s) => {
            ctx.log.line(&format!("installing from {} ({})", s.windows_path(), s.version));
            ctx.source = Some(s);
            Ok(())
        }
        Err(SourceError::Cancelled) => Err(Fail::cancelled()),
        Err(SourceError::Layout(m)) => {
            let line = format!("could not prepare the Office files ({m}); setup will download them itself, which is much slower");
            ctx.log.line(&line);
            (ctx.emit)(Event::Line(line));
            (ctx.emit)(Event::Detail("Setup will download the Office files itself".into()));
            Ok(())
        }
        Err(e) => Err(Fail::msg(format!(
            "Downloading the Office files failed: {e}\n\nStart the installation again to resume the download."
        ))),
    }
}

fn stage_service(ctx: &mut Ctx) -> Result<(), Fail> {
    let wine = ctx.wine();
    let emit = &mut *ctx.emit;
    let out = run(&odt::service_create_cmd(&wine), Some(&ctx.log), &mut |_, l| emit(Event::Line(l.into())), ctx.cancel)?;
    if odt::service_ok(out.status.code()) {
        Ok(())
    } else {
        Err(Fail {
            exit_code: out.status.code(),
            ..Fail::msg(format!("registering ClickToRunSvc failed:\n{}", out.tail(10)))
        })
    }
}

fn stage_setup(ctx: &mut Ctx) -> Result<(), Fail> {
    let win_source = ctx.source.as_ref().map(|s| (s.windows_path(), s.version.clone()));
    let xml = ctx
        .opts
        .deployment
        .render_with(win_source.as_ref().map(|(path, version)| InstallSource { path, version }));
    let win_config = config_xml::write_config(ctx.paths, &xml)?;
    ctx.log.line(&format!("Configuration.xml:\n{xml}"));
    let wine = ctx.wine();
    let cmd = wine.wine([r"C:\windows\temp\office-deployment-tool\setup.exe", "/configure", win_config]);
    let started = SystemTime::now() - Duration::from_secs(2);
    let mut follower = LogFollower::for_prefix(&ctx.paths.prefix(), LOG_DIR_IN_DRIVE_C, started);

    let (tx, rx) = mpsc::channel::<String>();
    let (log, cancel) = (ctx.log.clone(), ctx.cancel.clone());
    let worker = std::thread::spawn(move || {
        run(&cmd, Some(&log), &mut |_: Stream, l: &str| {
            let _ = tx.send(l.to_string());
        }, &cancel)
    });
    let mut last = None;
    loop {
        while let Ok(l) = rx.try_recv() {
            (ctx.emit)(Event::Line(l));
        }
        for l in follower.poll() {
            (ctx.emit)(Event::Line(l));
        }
        if follower.percent != last {
            last = follower.percent;
            (ctx.emit)(Event::Progress(last.map(|p| p as f32 / 100.0)));
        }
        if worker.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    for l in rx.try_iter().chain(follower.poll()) {
        (ctx.emit)(Event::Line(l));
    }
    let setup_log = follower.latest_file();
    let out = worker.join().map_err(|_| Fail::msg("the setup monitor thread panicked"))??;
    if out.status.success() {
        Ok(())
    } else {
        Err(Fail {
            message: format!("Office setup exited with {}", crate::core::proc::describe(out.status)),
            exit_code: out.status.code(),
            setup_log,
            cancelled: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_order_matches_the_spec() {
        let titles: Vec<&str> = Stage::ALL.iter().map(|s| s.title()).collect();
        assert_eq!(titles.len(), 11);
        assert_eq!(Stage::Deps.index(), 0);
        assert!(Stage::SelfCheck.index() < Stage::Prefix.index());
        assert!(Stage::Odt.index() < Stage::Source.index() && Stage::Source.index() < Stage::Setup.index());
        assert!(Stage::Service.index() < Stage::Setup.index());
        assert_eq!(Stage::Launchers.index(), 10);
    }
}

/// Live pipeline run in the test system: `cargo test -- --ignored live_pipeline --nocapture`.
/// With `LOIS_CANCEL_AT=<percent>` it cancels once setup reports that percentage and checks
/// that no wineserver is left.
#[cfg(test)]
mod live {
    use super::*;
    use crate::core::config::Config;

    #[test]
    #[ignore]
    fn live_pipeline() {
        let paths = Paths::from_env();
        let config = SharedConfig::new(Config::load(&paths.config_file()).config, paths.config_file());
        let opts = Options { deployment: DeploymentConfig::default(), install_deps: false, default_handlers: false, office_dpi: None, repair: false };
        let cancel = Cancel::new();
        let cancel_at: Option<f32> = std::env::var("LOIS_CANCEL_AT").ok().and_then(|v| v.parse().ok());
        let mut percents = Vec::new();
        let mut current = Stage::Deps;
        let c2 = cancel.clone();
        let outcome = run_install(&paths, &config, &opts, &cancel, &mut |e| {
            match &e {
                Event::Stage(s, st) if *st != Status::Pending => {
                    current = *s;
                    eprintln!("[stage] {} -> {st:?}", s.title());
                }
                Event::Detail(d) => eprintln!("[detail] {d}"),
                Event::Progress(Some(p)) if current == Stage::Setup => {
                    let pct = (p * 100.0).round();
                    if percents.last() != Some(&pct) {
                        percents.push(pct);
                        eprintln!("[setup] {pct}%");
                    }
                    if cancel_at.is_some_and(|c| pct >= c) {
                        c2.cancel();
                    }
                }
                _ => {}
            }
        });
        eprintln!("outcome: {outcome:?}");
        if cancel_at.is_some() {
            assert!(matches!(outcome, Outcome::Cancelled { stage: Stage::Setup, .. }));
            std::thread::sleep(Duration::from_secs(2));
            let pg = std::process::Command::new("pgrep").args(["-f", "wineserver"]).output().unwrap();
            let left = String::from_utf8_lossy(&pg.stdout).to_string();
            assert!(left.trim().is_empty(), "wineserver still running: {left}");
        } else {
            assert!(matches!(outcome, Outcome::Success { .. }), "{outcome:?}");
        }
        assert!(percents.len() > 1, "live percentages were reported: {percents:?}");
    }
}
