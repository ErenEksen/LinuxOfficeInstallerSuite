//! Rust <-> Slint bridge (design D2): pushes state into the `AppState` global, wires its
//! callbacks, and runs long operations on worker threads that report back through
//! `upgrade_in_event_loop`.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel, Weak};

use crate::core::config::{Config, SharedConfig, ThemePref};
use crate::core::paths::Paths;
use crate::core::proc::OpLog;
use crate::core::{Cancel, selfinstall};
use crate::deps::{self, DepReport, install::RepoStatus, os_release::SupportLevel};
use crate::desktop;
use crate::office::config_xml::{self, DeploymentConfig, DisplayLevel, Edition, OfficeApp};
use crate::office::detect::{self, App};
use crate::office::pipeline::{self, Event, Options, Outcome, Stage, Status};
use crate::office::plans::{Channel, PLAN_WARNING, Plan};
use crate::runner::{self, active, github};
use crate::{AppChoice, AppState, AppWindow, OfficeAppRow, Page, ReleaseRow, RunnerRow, StageRow, Theme, ThemeMode};

const LOG_LINES: usize = 400;

fn ss(s: impl AsRef<str>) -> SharedString {
    SharedString::from(s.as_ref())
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

fn home_relative(paths: &Paths, p: &Path) -> String {
    match p.strip_prefix(&paths.home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

/// Open a file or folder with the desktop's default handler, detached.
fn open_path(p: &str) {
    use std::os::unix::process::CommandExt;
    let _ = std::process::Command::new("xdg-open")
        .arg(p)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn();
}

#[derive(Clone)]
struct Ctx {
    paths: Paths,
    config: SharedConfig,
    weak: Weak<AppWindow>,
    cancel: Arc<Mutex<Option<Cancel>>>,
}

impl Ctx {
    /// Run `f` on the UI thread with the window (no-op if it is gone).
    fn ui(&self, f: impl FnOnce(&AppWindow) + Send + 'static) {
        let _ = self.weak.upgrade_in_event_loop(move |w| f(&w));
    }
}

/// Wire everything up. Called once on the UI thread before `run()`.
pub fn setup(window: &AppWindow, paths: Paths, config: Config, notice: Option<String>) {
    let ctx = Ctx {
        paths: paths.clone(),
        config: SharedConfig::new(config, paths.config_file()),
        weak: window.as_weak(),
        cancel: Arc::new(Mutex::new(None)),
    };
    let state = window.global::<AppState>();
    state.set_version(ss(env!("CARGO_PKG_VERSION")));
    state.set_notice(ss(notice.unwrap_or_default()));
    state.set_logs_dir(ss(home_relative(&paths, &paths.logs())));
    let cfg = ctx.config.get();
    apply_theme(window, cfg.theme);
    state.set_default_handlers(cfg.default_handlers);
    state.set_office_scales(model(crate::prefix::display::labels().into_iter().map(ss).collect()));
    state.set_office_scale_index(crate::prefix::display::index_of(cfg.office_dpi.unwrap_or(crate::prefix::display::DEFAULT_DPI)) as i32);
    state.set_office_installed(cfg.install.office_installed);
    state.set_save_path(ss(paths.home.join("Configuration.xml").to_string_lossy()));
    let exe = std::env::current_exe().unwrap_or_default();
    state.set_offer_self_install(!cfg.install.self_install_dismissed && !selfinstall::is_installed_copy(&paths, &exe));
    if std::env::var_os("LOIS_GALLERY").is_some() {
        state.set_page(Page::Gallery);
    }
    state.set_stages(model(
        Stage::ALL.iter().map(|s| StageRow { title: ss(s.title()), status: 0, detail: ss("") }).collect(),
    ));
    push_form(window, &ctx);
    refresh_apps(window, &ctx);
    refresh_runners(window, &ctx);
    start_deps_check(&ctx);

    // Closing the window during an install asks to cancel first, so no Wine process of the
    // prefix is left running without the app.
    let weak = window.as_weak();
    window.window().on_close_requested(move || {
        match weak.upgrade() {
            Some(w) if w.global::<AppState>().get_installing() => {
                w.global::<AppState>().set_dialog(ss("cancel-install"));
                slint::CloseRequestResponse::KeepWindowShown
            }
            _ => slint::CloseRequestResponse::HideWindow,
        }
    });

    super::chrome::wire(window);
    wire_misc(window, &ctx);
    wire_form(window, &ctx);
    wire_deps(window, &ctx);
    wire_install(window, &ctx);
    wire_apps(window, &ctx);
    wire_runners(window, &ctx);
    wire_settings(window, &ctx);
}

// ---------------------------------------------------------------- theme & misc

fn apply_theme(w: &AppWindow, t: ThemePref) {
    let (mode, idx) = match t {
        ThemePref::System => (ThemeMode::System, 0),
        ThemePref::Light => (ThemeMode::Light, 1),
        ThemePref::Dark => (ThemeMode::Dark, 2),
    };
    w.global::<Theme>().set_mode(mode);
    w.global::<AppState>().set_theme_index(idx);
}

fn wire_misc(w: &AppWindow, ctx: &Ctx) {
    let state = w.global::<AppState>();
    state.on_open_path(|p| open_path(&p));
    let c = ctx.clone();
    state.on_open_logs(move || {
        let _ = std::fs::create_dir_all(c.paths.logs());
        open_path(&c.paths.logs().to_string_lossy());
    });
    let weak = w.as_weak();
    state.on_dismiss_notice(move || {
        if let Some(w) = weak.upgrade() {
            w.global::<AppState>().set_notice(ss(""));
        }
    });
    let c = ctx.clone();
    state.on_dismiss_self_install(move || {
        c.config.update(|cfg| cfg.install.self_install_dismissed = true);
        c.ui(|w| w.global::<AppState>().set_offer_self_install(false));
    });
    let c = ctx.clone();
    state.on_self_install(move || {
        let exe = std::env::current_exe().unwrap_or_default();
        let result = selfinstall::install(&c.paths, &exe);
        c.ui(move |w| {
            let s = w.global::<AppState>();
            match result {
                Ok(()) => {
                    s.set_offer_self_install(false);
                    s.set_settings_message(ss("Linux Office Installer Suite is installed for this user and is in your application menu."));
                    s.set_page(Page::Settings);
                }
                Err(e) => s.set_self_install_message(ss(format!("Could not install: {e}"))),
            }
        });
    });
}

// ---------------------------------------------------------------- deployment form

fn push_form(w: &AppWindow, ctx: &Ctx) {
    let d = ctx.config.get().deployment;
    let s = w.global::<AppState>();
    s.set_plans(model(Plan::ALL.iter().map(|p| ss(p.label())).collect()));
    s.set_plan_index(Plan::ALL.iter().position(|p| *p == d.plan).unwrap_or(0) as i32);
    s.set_plan_warning(ss(PLAN_WARNING));
    s.set_channels(model(Channel::ALL.iter().map(|c| ss(c.odt_name())).collect()));
    let ch = d.plan.effective_channel(d.channel);
    s.set_channel_index(Channel::ALL.iter().position(|c| *c == ch).unwrap_or(0) as i32);
    s.set_channel_editable(d.plan.channel_editable());
    s.set_edition_index(if d.edition == Edition::X86 { 1 } else { 0 });
    s.set_apps(model(
        OfficeApp::ALL
            .iter()
            .map(|a| AppChoice { label: ss(a.label()), note: ss(a.known_issue().unwrap_or("")), checked: d.includes(*a) })
            .collect(),
    ));
    s.set_match_os(d.match_os);
    s.set_languages(ss(d.languages.join(", ")));
    s.set_updates_enabled(d.updates_enabled);
    s.set_display_full(d.display_level == DisplayLevel::Full);
    s.set_logging(d.logging);
    push_derived(w, ctx, &d);
}

/// XML preview and architecture check; never touches editable fields.
fn push_derived(w: &AppWindow, ctx: &Ctx, d: &DeploymentConfig) {
    let s = w.global::<AppState>();
    s.set_xml_preview(ss(d.render()));
    let err = config_xml::check_architecture(d, config_xml::installed_edition_in(&ctx.paths.prefix())).err();
    s.set_arch_error(ss(err.unwrap_or_default()));
}

fn wire_form(w: &AppWindow, ctx: &Ctx) {
    let state = w.global::<AppState>();
    let update = {
        let ctx = ctx.clone();
        move |full: bool, f: Box<dyn FnOnce(&mut DeploymentConfig)>| {
            let d = ctx.config.update(|c| {
                f(&mut c.deployment);
                c.deployment.channel = c.deployment.plan.effective_channel(c.deployment.channel);
                c.deployment.clone()
            });
            if let Some(w) = ctx.weak.upgrade() {
                if full { push_form(&w, &ctx) } else { push_derived(&w, &ctx, &d) }
            }
        }
    };
    let u = update.clone();
    state.on_set_plan(move |i| u(true, Box::new(move |d| d.plan = Plan::ALL[i.clamp(0, 8) as usize])));
    let u = update.clone();
    state.on_set_channel(move |i| u(true, Box::new(move |d| d.channel = Channel::ALL[i.clamp(0, 2) as usize])));
    let u = update.clone();
    state.on_set_edition(move |i| u(true, Box::new(move |d| d.edition = if i == 1 { Edition::X86 } else { Edition::X64 })));
    let u = update.clone();
    state.on_set_app(move |i, on| u(true, Box::new(move |d| d.set_app(OfficeApp::ALL[i as usize], on))));
    let u = update.clone();
    let weak = w.as_weak();
    state.on_form_changed(move || {
        let Some(w) = weak.upgrade() else { return };
        let s = w.global::<AppState>();
        let (match_os, langs, upd, full, logging) =
            (s.get_match_os(), s.get_languages(), s.get_updates_enabled(), s.get_display_full(), s.get_logging());
        u(false, Box::new(move |d| {
            d.match_os = match_os;
            d.languages = langs.split(',').map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
            d.updates_enabled = upd;
            d.display_level = if full { DisplayLevel::Full } else { DisplayLevel::None };
            d.logging = logging;
        }));
    });
    let c = ctx.clone();
    state.on_save_xml(move || {
        let Some(w) = c.weak.upgrade() else { return };
        let s = w.global::<AppState>();
        let raw = s.get_save_path().to_string();
        let path = match raw.strip_prefix("~/") {
            Some(rest) => c.paths.home.join(rest),
            None => PathBuf::from(&raw),
        };
        let xml = c.config.get().deployment.render();
        let msg = match crate::core::config::write_atomic(&path, xml.as_bytes()) {
            Ok(()) => format!("Saved to {}", path.display()),
            Err(e) => format!("Could not save: {e}"),
        };
        s.set_save_result(ss(msg));
    });
}

// ---------------------------------------------------------------- dependencies

fn push_deps(w: &AppWindow, r: &DepReport) {
    let s = w.global::<AppState>();
    s.set_deps_checked(true);
    s.set_deps_busy(false);
    s.set_deps_ready(r.ready());
    s.set_distro_name(ss(&r.os.pretty_name));
    let level = r.family().support();
    s.set_support_level(ss(level.label()));
    s.set_support_untested(level == SupportLevel::Untested);
    let mut expl = r.support_explanation().to_string();
    if level == SupportLevel::Untested {
        expl.push(' ');
        expl.push_str(deps::maps::UNTESTED_NOTE);
    }
    s.set_support_explanation(ss(expl));
    s.set_deps_missing(model(r.missing.iter().map(|m| ss(m.line())).collect()));
    s.set_deps_undetected(r.undetected.is_some());
    s.set_deps_undetected_title(ss(r.undetected.map(|u| u.title()).unwrap_or_default()));
    s.set_deps_undetected_message(ss(r.undetected.map(|u| u.message()).unwrap_or_default()));
    s.set_deps_copy_text(ss(r.copy_text()));
    let pkgs = r.packages();
    s.set_deps_packages(ss(pkgs.join(" ")));
    s.set_deps_command(ss(deps::install::install_command(r.family(), &pkgs)
        .map(|c| deps::install::copyable_command(&c))
        .unwrap_or_default()));
    s.set_repo_instructions(ss(match &r.repo {
        RepoStatus::Disabled { instructions } if !r.ready() => instructions.clone(),
        _ => String::new(),
    }));
    s.set_deps_can_auto_install(r.can_auto_install());
}

fn start_deps_check(ctx: &Ctx) {
    let c = ctx.clone();
    c.ui(|w| w.global::<AppState>().set_deps_busy(true));
    std::thread::spawn(move || {
        let report = deps::check();
        c.ui(move |w| push_deps(w, &report));
    });
}

fn wire_deps(w: &AppWindow, ctx: &Ctx) {
    let state = w.global::<AppState>();
    let c = ctx.clone();
    state.on_check_deps(move || start_deps_check(&c));
    let c = ctx.clone();
    state.on_install_packages(move || {
        let c = c.clone();
        c.ui(|w| {
            let s = w.global::<AppState>();
            s.set_deps_busy(true);
            s.set_deps_error(ss(""));
        });
        std::thread::spawn(move || {
            let report = deps::check();
            let result = OpLog::create(&c.paths.logs(), "deps-install").map_err(|e| e.to_string()).and_then(|log| {
                deps::install::run_install(report.family(), &report.packages(), &log, &mut |_, _| {}, &Cancel::new())
                    .map_err(|e| format!("{e}\nLog: {}", log.path().display()))
            });
            let after = deps::check();
            c.ui(move |w| {
                push_deps(w, &after);
                if let Err(e) = result {
                    w.global::<AppState>().set_deps_error(ss(e));
                }
            });
        });
    });
}

// ---------------------------------------------------------------- install pipeline

#[derive(Default)]
struct Pending {
    statuses: Vec<(i32, String)>,
    progress: Option<f32>,
    detail: String,
    log: VecDeque<String>,
    log_dirty: bool,
    scheduled: bool,
}

fn status_code(s: Status) -> i32 {
    match s {
        Status::Pending => 0,
        Status::Running => 1,
        Status::Done => 2,
        Status::Failed => 3,
        Status::Cancelled => 4,
    }
}

fn flush(w: &AppWindow, pending: &Mutex<Pending>) {
    let mut p = pending.lock().unwrap();
    p.scheduled = false;
    let s = w.global::<AppState>();
    s.set_stages(model(
        Stage::ALL
            .iter()
            .zip(&p.statuses)
            .map(|(st, (code, detail))| StageRow { title: ss(st.title()), status: *code, detail: ss(detail) })
            .collect(),
    ));
    s.set_stage_indeterminate(p.progress.is_none());
    s.set_stage_progress(p.progress.unwrap_or(0.0));
    s.set_stage_detail(ss(&p.detail));
    if p.log_dirty {
        p.log_dirty = false;
        let text: Vec<&str> = p.log.iter().map(String::as_str).collect();
        s.set_log_text(ss(text.join("\n")));
    }
}

fn start_install(ctx: &Ctx, install_deps: bool, repair: bool) {
    let Some(w) = ctx.weak.upgrade() else { return };
    let s = w.global::<AppState>();
    if s.get_installing() {
        return;
    }
    s.set_installing(true);
    s.set_install_ever_started(true);
    s.set_result_kind(0);
    s.set_log_text(ss(""));
    s.set_page(Page::Progress);
    let cancel = Cancel::new();
    *ctx.cancel.lock().unwrap() = Some(cancel.clone());
    let cfg = ctx.config.get();
    let opts = Options {
        deployment: cfg.deployment.clone(),
        install_deps,
        default_handlers: cfg.default_handlers,
        office_dpi: cfg.office_dpi,
        repair,
    };
    let c = ctx.clone();
    std::thread::spawn(move || {
        let pending = Arc::new(Mutex::new(Pending {
            statuses: vec![(0, String::new()); Stage::ALL.len()],
            ..Default::default()
        }));
        let mut current = 0usize;
        let mut emit = |e: Event| {
            let mut p = pending.lock().unwrap();
            match e {
                Event::Stage(stage, st) => {
                    current = stage.index();
                    p.statuses[current].0 = status_code(st);
                    if st == Status::Running {
                        p.detail.clear();
                        p.statuses[current].1.clear();
                    }
                }
                Event::Progress(f) => p.progress = f,
                Event::Detail(d) => {
                    p.statuses[current].1 = d.clone();
                    p.detail = d;
                }
                Event::Line(l) => {
                    if p.log.len() >= LOG_LINES {
                        p.log.pop_front();
                    }
                    // Click-to-Run logs are tab-separated; show tabs as spaces, drop other controls.
                    let clean: String = l
                        .chars()
                        .map(|c| if c == '\t' { ' ' } else { c })
                        .filter(|c| !c.is_control())
                        .collect();
                    p.log.push_back(clean);
                    p.log_dirty = true;
                }
            }
            if !p.scheduled {
                p.scheduled = true;
                let pend = pending.clone();
                c.ui(move |w| flush(w, &pend));
            }
        };
        let outcome = pipeline::run_install(&c.paths, &c.config, &opts, &cancel, &mut emit);
        let pend = pending.clone();
        let c2 = c.clone();
        c.ui(move |w| {
            flush(w, &pend);
            finish_install(w, &c2, outcome);
        });
    });
}

fn finish_install(w: &AppWindow, ctx: &Ctx, outcome: Outcome) {
    *ctx.cancel.lock().unwrap() = None;
    let s = w.global::<AppState>();
    s.set_installing(false);
    match outcome {
        Outcome::Success { log, generic_icons, .. } => {
            s.set_result_kind(1);
            s.set_result_log(ss(log.to_string_lossy()));
            if !generic_icons.is_empty() {
                s.set_apps_message(ss(format!(
                    "No icon could be extracted for {}; a generic icon is used.",
                    generic_icons.iter().map(|a| a.name()).collect::<Vec<_>>().join(", ")
                )));
            }
        }
        Outcome::Failed { stage, message, exit_code, log, setup_log } => {
            s.set_result_kind(2);
            s.set_result_title(ss(format!("{} failed", stage.title())));
            let mut m = message;
            if let Some(code) = exit_code {
                m.push_str(&format!("\nExit code: {code}"));
            }
            if let Some(sl) = setup_log {
                m.push_str(&format!("\nSetup log: {}", sl.display()));
            }
            m.push_str(&format!("\nLog: {}", log.display()));
            s.set_result_message(ss(m));
            s.set_result_log(ss(log.to_string_lossy()));
        }
        Outcome::Cancelled { stage, log } => {
            s.set_result_kind(3);
            s.set_result_message(ss(format!(
                "Stopped during \"{}\". All Wine processes of the prefix were stopped; finished steps are kept.",
                stage.title()
            )));
            s.set_result_log(ss(log.to_string_lossy()));
        }
    }
    s.set_office_installed(ctx.config.get().install.office_installed);
    refresh_apps(w, ctx);
    refresh_runners(w, ctx);
    push_derived(w, ctx, &ctx.config.get().deployment);
    start_deps_check(ctx);
}

fn wire_install(w: &AppWindow, ctx: &Ctx) {
    let state = w.global::<AppState>();
    let c = ctx.clone();
    state.on_start_install(move || start_install(&c, false, false));
    let c = ctx.clone();
    state.on_confirm_install_with_packages(move || start_install(&c, true, false));
    let c = ctx.clone();
    state.on_repair_office(move || {
        // Office has to be closed while its prefix changes.
        let runner = c.paths.active_runner_link();
        if runner.exists() {
            crate::prefix::wine::Wine::new(runner, c.paths.prefix()).kill_all();
        }
        start_install(&c, false, true);
    });
    let c = ctx.clone();
    state.on_cancel_install(move || {
        if let Some(cancel) = c.cancel.lock().unwrap().as_ref() {
            cancel.cancel();
        }
    });
}

// ---------------------------------------------------------------- Office apps

fn app_icon(paths: &Paths, app: App) -> Image {
    let p = paths.icons_hicolor().join(format!("48x48/apps/{}.png", desktop::entries::icon_name(app)));
    Image::load_from_path(&p).unwrap_or_default()
}

/// Whether an existing installation lacks parts added in later versions: prefix steps
/// (such as the web components for sign-in), the sign-in helper or the current launcher.
fn needs_repair(paths: &Paths, office_installed: bool) -> bool {
    let launcher_current = std::fs::read_to_string(paths.launch_wrapper())
        .is_ok_and(|s| s.contains(crate::identity::server::SOCKET_ENV) && s.contains("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"));
    office_installed
        && (!crate::prefix::setup_complete(paths) || !crate::identity::helper_path(paths).exists() || !launcher_current)
}

fn refresh_apps(w: &AppWindow, ctx: &Ctx) {
    w.global::<AppState>().set_office_needs_repair(needs_repair(&ctx.paths, ctx.config.get().install.office_installed));
    let apps = detect::detect(&ctx.paths.prefix());
    w.global::<AppState>().set_office_apps(model(
        apps.iter()
            .map(|a| OfficeAppRow { id: ss(a.id()), name: ss(a.name()), exe: ss(a.exe()), icon: app_icon(&ctx.paths, *a) })
            .collect(),
    ));
}

fn wire_apps(w: &AppWindow, ctx: &Ctx) {
    let state = w.global::<AppState>();
    let c = ctx.clone();
    state.on_refresh_apps(move || {
        if let Some(w) = c.weak.upgrade() {
            refresh_apps(&w, &c);
        }
    });
    let c = ctx.clone();
    state.on_launch_app(move |id| {
        let Some(app) = App::from_id(&id) else { return };
        let msg = if !desktop::launcher::exists(&c.paths) {
            "The launcher is missing. Use Recreate launcher first.".to_string()
        } else {
            match desktop::launch(&c.paths, app, &[]) {
                Ok(()) => format!("Starting {}…", app.name()),
                Err(e) => format!("Could not start {}: {e}", app.name()),
            }
        };
        c.ui(move |w| w.global::<AppState>().set_apps_message(ss(msg)));
    });
    let c = ctx.clone();
    state.on_stop_office_apps(move || {
        let runner = c.paths.active_runner_link();
        if !runner.join("bin/wineserver").exists() {
            c.ui(|w| w.global::<AppState>().set_apps_message(ss("No active Wine runner, so no Office apps can be running.")));
            return;
        }
        c.ui(|w| w.global::<AppState>().set_apps_message(ss("Stopping all Office apps…")));
        let c = c.clone();
        std::thread::spawn(move || {
            crate::prefix::wine::Wine::new(runner, c.paths.prefix()).kill_all();
            c.ui(|w| w.global::<AppState>().set_apps_message(ss("All Office apps were stopped.")));
        });
    });
    let c = ctx.clone();
    state.on_recreate_launcher(move |id| {
        let Some(app) = App::from_id(&id) else { return };
        let msg = match desktop::recreate_launcher(&c.paths, app, &Cancel::new()) {
            Ok(()) => format!("The launcher for {} was written again.", app.name()),
            Err(e) => format!("Could not write the launcher: {e}"),
        };
        let c2 = c.clone();
        c.ui(move |w| {
            w.global::<AppState>().set_apps_message(ss(msg));
            refresh_apps(w, &c2);
        });
    });
    let c = ctx.clone();
    state.on_uninstall_office(move |delete_prefix| {
        let c = c.clone();
        std::thread::spawn(move || {
            let msg = match crate::office::uninstall_office(&c.paths, &c.config, delete_prefix) {
                Ok(()) if delete_prefix => "Office was uninstalled and the prefix deleted.".to_string(),
                Ok(()) => "The Office launchers were removed. The prefix was kept.".to_string(),
                Err(e) => format!("Uninstall failed: {e}"),
            };
            let c2 = c.clone();
            c.ui(move |w| {
                let s = w.global::<AppState>();
                s.set_apps_message(ss(msg));
                s.set_office_installed(c2.config.get().install.office_installed);
                refresh_apps(w, &c2);
                push_derived(w, &c2, &c2.config.get().deployment);
            });
        });
    });
}

// ---------------------------------------------------------------- Wine runtimes

fn refresh_runners(w: &AppWindow, ctx: &Ctx) {
    let mut cfg = ctx.config.get();
    let act = active::reconcile(&ctx.paths, &mut cfg);
    if cfg.active_runner != ctx.config.get().active_runner {
        let a = cfg.active_runner.clone();
        ctx.config.update(|c| c.active_runner = a);
    }
    w.global::<AppState>().set_runners(model(
        active::installed(&ctx.paths)
            .into_iter()
            .map(|tag| {
                let supported = crate::runner::supports_office(&tag);
                let location = home_relative(&ctx.paths, &ctx.paths.runner(&tag));
                let mut subtitle =
                    if act.as_deref() == Some(tag.as_str()) { format!("Active · {location}") } else { location };
                if !supported {
                    subtitle.push_str(&format!(" · {}", crate::runner::TOO_OLD));
                }
                RunnerRow { subtitle: ss(subtitle), active: act.as_deref() == Some(tag.as_str()), supported, tag: ss(tag) }
            })
            .collect(),
    ));
}

fn wire_runners(w: &AppWindow, ctx: &Ctx) {
    let state = w.global::<AppState>();
    let releases: Arc<Mutex<Vec<github::Release>>> = Arc::default();
    let c = ctx.clone();
    state.on_refresh_runners(move || {
        if let Some(w) = c.weak.upgrade() {
            refresh_runners(&w, &c);
        }
    });
    let c = ctx.clone();
    let rel = releases.clone();
    state.on_load_releases(move || {
        let (c, rel) = (c.clone(), rel.clone());
        c.ui(|w| {
            let s = w.global::<AppState>();
            s.set_releases_loading(true);
            s.set_releases_error(ss(""));
        });
        std::thread::spawn(move || {
            let result = github::fetch_releases(&crate::core::download::http_client(), github::API_URL);
            let installed = active::installed(&c.paths);
            c.ui(move |w| {
                let s = w.global::<AppState>();
                s.set_releases_loading(false);
                match result {
                    Ok(list) => {
                        s.set_releases(model(
                            list.iter()
                                .map(|r| ReleaseRow {
                                    tag: ss(&r.tag),
                                    subtitle: ss(format!(
                                        "{}{}{}{}",
                                        r.date,
                                        if r.prerelease { " · Prerelease" } else { "" },
                                        if r.sums_url.is_none() { " · No checksums, cannot be verified" } else { "" },
                                        if crate::runner::supports_office(&r.tag) {
                                            String::new()
                                        } else {
                                            format!(" · {}", crate::runner::TOO_OLD)
                                        }
                                    )),
                                    installed: installed.contains(&r.tag),
                                    installable: r.sums_url.is_some() && crate::runner::supports_office(&r.tag),
                                })
                                .collect(),
                        ));
                        *rel.lock().unwrap() = list;
                    }
                    Err(e) => s.set_releases_error(ss(e.to_string())),
                }
            });
        });
    });
    let c = ctx.clone();
    let rel = releases.clone();
    state.on_install_release(move |tag| {
        let Some(release) = rel.lock().unwrap().iter().find(|r| r.tag == tag.as_str()).cloned() else { return };
        if !crate::runner::supports_office(&release.tag) {
            return;
        }
        let c = c.clone();
        c.ui(move |w| {
            let s = w.global::<AppState>();
            s.set_runner_busy(true);
            s.set_runner_error(ss(""));
            s.set_runner_progress(0.0);
            s.set_runner_status(ss(format!("Downloading {tag}…")));
        });
        std::thread::spawn(move || {
            let log = OpLog::create(&c.paths.logs(), "runner-install").ok();
            let weak = c.weak.clone();
            let mut last = 0.0f32;
            let result = runner::install::install_runner(
                &crate::core::download::http_client(),
                &c.paths,
                &release,
                &mut |phase, d, t| {
                    let f = t.filter(|t| *t > 0).map(|t| d as f32 / t as f32).unwrap_or(0.0);
                    if (f - last).abs() < 0.01 && f < 1.0 {
                        return;
                    }
                    last = f;
                    let label = match phase {
                        runner::install::Phase::Download => "Downloading",
                        runner::install::Phase::Extract => "Extracting",
                    };
                    let tag = release.tag.clone();
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        let s = w.global::<AppState>();
                        s.set_runner_progress(f);
                        s.set_runner_status(ss(format!("{label} {tag}… {:.0} %", f * 100.0)));
                    });
                },
                log.as_ref(),
                &Cancel::new(),
            );
            if result.is_ok() && active::active(&c.paths).is_none() {
                let mut cfg = c.config.get();
                if active::set_active(&c.paths, &mut cfg, &release.tag).is_ok() {
                    c.config.update(|x| x.active_runner = cfg.active_runner.clone());
                }
            }
            let c2 = c.clone();
            c.ui(move |w| {
                let s = w.global::<AppState>();
                s.set_runner_busy(false);
                if let Err(e) = result {
                    s.set_runner_error(ss(e.to_string()));
                }
                refresh_runners(w, &c2);
            });
        });
    });
    let c = ctx.clone();
    state.on_set_active_runner(move |tag| {
        if !crate::runner::supports_office(&tag) {
            let c2 = c.clone();
            c.ui(move |w| {
                w.global::<AppState>().set_runner_error(ss(format!("{tag}: {}", crate::runner::TOO_OLD)));
                refresh_runners(w, &c2);
            });
            return;
        }
        let mut cfg = c.config.get();
        let r = active::set_active(&c.paths, &mut cfg, &tag);
        c.config.update(|x| x.active_runner = cfg.active_runner.clone());
        let c2 = c.clone();
        c.ui(move |w| {
            w.global::<AppState>().set_runner_error(ss(r.err().map(|e| e.to_string()).unwrap_or_default()));
            refresh_runners(w, &c2);
        });
    });
    let c = ctx.clone();
    state.on_remove_runner(move |tag| {
        let c = c.clone();
        c.ui(|w| {
            let s = w.global::<AppState>();
            s.set_runner_busy(true);
            s.set_runner_progress(0.0);
            s.set_runner_status(ss("Removing…"));
        });
        std::thread::spawn(move || {
            let r = active::remove(&c.paths, &tag);
            let c2 = c.clone();
            c.ui(move |w| {
                let s = w.global::<AppState>();
                s.set_runner_busy(false);
                s.set_runner_error(ss(r.err().map(|e| e.to_string()).unwrap_or_default()));
                refresh_runners(w, &c2);
            });
        });
    });
}

// ---------------------------------------------------------------- settings

fn wire_settings(w: &AppWindow, ctx: &Ctx) {
    let state = w.global::<AppState>();
    let c = ctx.clone();
    state.on_set_theme(move |i| {
        let t = match i {
            1 => ThemePref::Light,
            2 => ThemePref::Dark,
            _ => ThemePref::System,
        };
        c.config.update(|cfg| cfg.theme = t);
        if let Some(w) = c.weak.upgrade() {
            apply_theme(&w, t);
        }
    });
    let c = ctx.clone();
    state.on_set_default_handlers(move |on| {
        c.config.update(|cfg| cfg.default_handlers = on);
        let apps = detect::detect(&c.paths.prefix());
        let result: std::io::Result<String> = if apps.is_empty() || !desktop::launcher::exists(&c.paths) {
            Ok(if on {
                "Office apps will be set as defaults when Office is installed.".into()
            } else {
                String::new()
            })
        } else if on {
            let mut ledger = crate::core::ledger::Ledger::load(&c.paths.ledger_file());
            apps.iter()
                .try_for_each(|a| {
                    desktop::mime::apply_defaults(
                        &c.paths.mimeapps(),
                        &mut ledger,
                        &desktop::entries::desktop_id(*a),
                        a.mime_types(),
                    )
                })
                .map(|_| "Office apps are now the default for their file types.".into())
        } else {
            let mut ledger = crate::core::ledger::Ledger::load(&c.paths.ledger_file());
            desktop::mime::restore_defaults(&c.paths.mimeapps(), &mut ledger)
                .map(|_| "Your previous default apps were restored.".into())
        };
        let msg = result.unwrap_or_else(|e| format!("Could not change default apps: {e}"));
        c.ui(move |w| {
            let s = w.global::<AppState>();
            s.set_default_handlers(on);
            s.set_settings_message(ss(msg));
        });
    });
    let c = ctx.clone();
    state.on_set_office_scale(move |i| {
        let Some(&(pct, dpi)) = crate::prefix::display::SCALES.get(i.max(0) as usize) else { return };
        c.config.update(|cfg| cfg.office_dpi = Some(dpi));
        let c = c.clone();
        std::thread::spawn(move || {
            let wine = crate::prefix::wine::Wine::new(c.paths.active_runner_link(), c.paths.prefix());
            let msg = if !c.paths.active_runner_link().exists() || !wine.is_initialised() {
                format!("Office will use {pct}% scaling once it is installed.")
            } else {
                match wine.reg_import(&crate::prefix::display::dpi_entries(dpi), None, &Cancel::new()) {
                    Ok(()) => format!("Office uses {pct}% scaling from the next time it starts. Close all Office apps first."),
                    Err(e) => format!("Could not set the display scaling: {e}"),
                }
            };
            c.ui(move |w| w.global::<AppState>().set_settings_message(ss(msg)));
        });
    });
    let c = ctx.clone();
    state.on_reset_prefix(move || {
        let c = c.clone();
        std::thread::spawn(move || {
            let runner = c.paths.active_runner_link();
            let r = crate::prefix::reset(&c.paths, runner.exists().then_some(runner.as_path()));
            if r.is_ok() {
                c.config.update(|cfg| {
                    cfg.install.office_installed = false;
                    cfg.install.product_id = None;
                });
            }
            let c2 = c.clone();
            c.ui(move |w| {
                let s = w.global::<AppState>();
                s.set_settings_message(ss(match r {
                    Ok(()) => "The prefix and the Office launchers were removed. Runners and downloads were kept.".into(),
                    Err(e) => format!("Reset failed: {e}"),
                }));
                s.set_office_installed(false);
                refresh_apps(w, &c2);
                push_derived(w, &c2, &c2.config.get().deployment);
            });
        });
    });
    let c = ctx.clone();
    state.on_remove_app(move |remove_office| {
        match selfinstall::uninstall(&c.paths, remove_office) {
            Ok(()) => {
                // Our config directory is gone; do not write it again on the way out.
                let _ = slint::quit_event_loop();
            }
            Err(e) => {
                let msg = format!("Removal failed: {e}");
                c.ui(move |w| w.global::<AppState>().set_settings_message(ss(msg)));
            }
        }
    });
    let c = ctx.clone();
    state.on_remove_everything(move || {
        c.ui(|w| {
            let s = w.global::<AppState>();
            s.set_removing(true);
            s.set_settings_message(ss("Removing everything. Closing Office apps and deleting ~/.msoffice can take a minute."));
        });
        let c = c.clone();
        // Deleting a multi-GB prefix must not block the window.
        std::thread::spawn(move || match selfinstall::remove_everything(&c.paths) {
            // Our config directory is gone; do not write it again on the way out.
            Ok(()) => c.ui(|_| {
                let _ = slint::quit_event_loop();
            }),
            Err(e) => {
                let msg = format!("Removal stopped: {e}. Run Remove everything again to finish.");
                c.ui(move |w| {
                    let s = w.global::<AppState>();
                    s.set_removing(false);
                    s.set_settings_message(ss(msg));
                });
            }
        });
    });
}
