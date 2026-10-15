//! myous for Linux and Windows: the human's side of a worker on this
//! computer. This crate is the thin part under the window: the worker
//! folders and their files, the container runtime (or, where there is
//! none, the worker the app carries, run directly), the agents on this
//! computer, the tray icon, notifications, the clipboard, dialogs and
//! launch at login. The screens, the state machine and the tray menu's
//! contents live in the window (ui/src), which drives this through
//! commands and listens to events.

mod agents;
mod browser;
mod containers;
mod direct;
mod engine;
mod files;
mod hub;
mod install;
#[cfg(target_os = "macos")]
mod loginitem;
mod paths;
mod platform;
mod toast;
mod tray;
#[cfg(target_os = "macos")]
mod usernotify;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, Runtime, WindowEvent};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_clipboard_manager::ClipboardExt as _;
use tauri_plugin_dialog::DialogExt as _;
use tauri_plugin_notification::NotificationExt as _;

/// The compose file for the published image, written into the worker
/// folder at start with the version filled in.
const COMPOSE_TEMPLATE: &str = include_str!("../../compose.yml");

const TRAY_ID: &str = "main";

/// Set when the window's script has started (it asks for app_info first).
static PAGE_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// The image tag the compose file names: the app's version, or
/// MYOUS_IMAGE_TAG (a development override).
fn image_tag() -> String {
    match std::env::var("MYOUS_IMAGE_TAG") {
        Ok(t) if !t.is_empty() => t,
        _ => version(),
    }
}

#[derive(Serialize)]
struct AppInfo {
    version: String,
    os: String,
    fake: String,
    system_name: String,
    product_name: String,
    download_path: String,
    hostname: String,
    hub: String,
    image_tag: String,
}

#[tauri::command]
fn app_info() -> AppInfo {
    PAGE_READY.store(true, std::sync::atomic::Ordering::SeqCst);
    AppInfo {
        version: version(),
        os: platform::os().into(),
        fake: std::env::var("MYOUS_FAKE_STATE").unwrap_or_default(),
        system_name: platform::system_name().into(),
        product_name: platform::product_name().into(),
        download_path: platform::download_path().into(),
        hostname: platform::hostname(),
        hub: hub::url(),
        image_tag: image_tag(),
    }
}

#[tauri::command]
fn read_workers() -> Vec<files::WorkerFiles> {
    let workers: Vec<files::WorkerFiles> = paths::all_homes()
        .iter()
        .map(|h| files::read_worker(h))
        .collect();
    // A Mac: a question answered elsewhere or timed out (the worker took
    // it away) takes its notification with it.
    #[cfg(target_os = "macos")]
    usernotify::withdraw_gone(|home, id| {
        workers.iter().any(|w| {
            w.home == home
                && w.approvals
                    .iter()
                    .any(|q| q.get("id").and_then(Value::as_str) == Some(id))
        })
    });
    workers
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[tauri::command]
fn write_config(home: String, config: Value) -> Result<(), String> {
    files::write_config(Path::new(&home), &config).map_err(err)
}

#[tauri::command]
fn send_command(home: String, name: String) -> Result<(), String> {
    if !files::valid_command(&name) {
        return Err(format!("not a worker command: {name:?}"));
    }
    paths::send_command(Path::new(&home), &name).map_err(err)
}

#[tauri::command]
fn set_paused(home: String, on: bool) -> Result<(), String> {
    paths::set_paused(Path::new(&home), on).map_err(err)
}

#[tauri::command]
fn answer_approval(home: String, id: String, verdict: String) -> Result<(), String> {
    if verdict != "allow" && verdict != "refuse" {
        return Err("verdict must be allow or refuse".into());
    }
    files::answer_approval(Path::new(&home), &id, &verdict).map_err(err)?;
    // A Mac: the question's notification, with its Allow and Refuse, goes.
    #[cfg(target_os = "macos")]
    usernotify::answered(&id);
    Ok(())
}

#[tauri::command]
fn set_review(home: String, level: String) -> Result<(), String> {
    if !["trust", "changes", "all"].contains(&level.as_str()) {
        return Err("unknown review level".into());
    }
    files::set_review_level(Path::new(&home), &level).map_err(err)
}

/// Creates the next `<default>-<n>` folder with an empty app.json and
/// returns its path.
#[tauri::command]
fn new_worker() -> Result<String, String> {
    let home = paths::new_home().ok_or("no free folder name")?;
    paths::ensure(&home).map_err(err)?;
    files::write_config(&home, &serde_json::json!({})).map_err(err)?;
    Ok(home.to_string_lossy().to_string())
}

/// Moves a worker's folder to the trash (never the first worker's).
#[tauri::command]
fn remove_worker(home: String) -> Result<(), String> {
    let p = PathBuf::from(&home);
    if paths::is_default(&p) {
        return Err("the first worker stays".into());
    }
    if !paths::all_homes().iter().any(|h| h == &p) {
        return Err("not a worker folder".into());
    }
    if trash::delete(&p).is_err() {
        std::fs::remove_dir_all(&p).map_err(err)?;
    }
    Ok(())
}

/// The runtime's state for the Set up card (engine.rs: ok, missing,
/// stopped, permission, nocompose, error, unknown).
#[tauri::command]
async fn probe_runtime() -> engine::RuntimeCheck {
    tauri::async_runtime::spawn_blocking(engine::check)
        .await
        .unwrap_or(engine::RuntimeCheck {
            state: "unknown".into(),
            name: "Docker".into(),
            detail: "the check crashed".into(),
        })
}

#[tauri::command]
fn launch_runtime(name: String) {
    engine::launch(&name)
}

/// What the Set up card can offer here: the runtimes in order, and for
/// each whether the app can install it; starting Docker Engine; the
/// group; compose (install.rs). `runtime` is the one found, if any.
#[tauri::command]
async fn runtime_options(runtime: String) -> install::Options {
    tauri::async_runtime::spawn_blocking(move || install::options(&install::detect(), &runtime))
        .await
        .unwrap_or_default()
}

#[derive(Serialize, Clone)]
struct InstallDone {
    id: String,
    status: i32,
}

/// Runs what the window asked for (a runtime's install, "start" or
/// "compose"), its output streamed into the app log as `app-log` events
/// (the first worker's), ending with an `install-done` event. The command
/// is built here from the id, never taken from the window.
#[tauri::command]
async fn runtime_install<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    runtime: String,
) -> Result<(), String> {
    let opts = tauri::async_runtime::spawn_blocking(move || {
        install::options(&install::detect(), &runtime)
    })
    .await
    .map_err(err)?;
    let argv = install::command(&opts, &id).ok_or("the app can't do that on this computer")?;
    let home = paths::all_homes()
        .first()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let log = move |a: &AppHandle<R>, line: String| {
        let _ = a.emit(
            "app-log",
            LogLine {
                home: home.clone(),
                line,
            },
        );
    };
    log(&app, format!("{id}: {}", argv.join(" ")));
    std::thread::spawn(move || {
        let a = app.clone();
        let status = engine::run_streaming(&argv, &std::env::temp_dir(), &HashMap::new(), |line| {
            log(&a, line)
        });
        let _ = app.emit("install-done", InstallDone { id, status });
    });
    Ok(())
}

/// Whether this computer reaches the hub (one GET, 5 s at most).
#[tauri::command]
async fn hub_check() -> hub::HubCheck {
    tauri::async_runtime::spawn_blocking(hub::check)
        .await
        .unwrap_or(hub::HubCheck {
            state: "down".into(),
            detail: "the check crashed".into(),
        })
}

/// Whether this computer can run containers at all (WSL 2 on Windows).
#[tauri::command]
async fn containers_check() -> containers::Capability {
    tauri::async_runtime::spawn_blocking(containers::check)
        .await
        .unwrap_or(containers::Capability {
            state: "ok".into(),
            reason: String::new(),
            machine: String::new(),
        })
}

/// Turns WSL on; Windows asks for an administrator, and a restart finishes it.
#[tauri::command]
async fn enable_wsl() -> Outcome {
    let (status, output) = tauri::async_runtime::spawn_blocking(containers::enable_wsl)
        .await
        .unwrap_or((-1, "could not run".into()));
    Outcome { status, output }
}

#[derive(Serialize, Debug)]
struct DirectStarted {
    pid: u32,
    /// The worker's browser: "" (not asked for), "starting", or why not.
    browser: String,
}

/// Starts a worker without a container (the bundled `myous-worker`, or
/// $MYOUS_WORKER_BIN); its exit is a `compose-done` event with op
/// "direct-exit", like a compose run's end. With `browser`, its browser
/// on this computer first (browser.rs); the worker's help names it from
/// browser.json.
#[tauri::command]
async fn direct_start<R: Runtime>(
    app: AppHandle<R>,
    home: String,
    name: String,
    description: String,
    browser: bool,
) -> Result<DirectStarted, String> {
    let (bin, overridden) = direct::worker_bin()?;
    let log = app_logger(&app, &home);
    if overridden {
        log(format!(
            "MYOUS_WORKER_BIN: running {} instead of the bundled worker",
            bin.display()
        ));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let h = home.clone();
        start_with_browser(
            browser,
            || direct::running(&home),
            || browser::start(Path::new(&home), &name, Vec::new(), log.clone()),
            || {
                // No --note for the browser: the worker reads browser.json at each help.
                let args = direct::worker_args(Path::new(&home), &bin, &name, &description, &[]);
                log(format!("start: {} (output in worker.log)", args.join(" ")));
                direct::start(&home, &args, move |status| {
                    // The browser lives and dies with the worker.
                    browser::stop(Path::new(&h));
                    let _ = app.emit(
                        "compose-done",
                        Done {
                            home: h,
                            op: "direct-exit".into(),
                            status,
                        },
                    );
                })
            },
            || browser::stop(Path::new(&home)),
            &log,
        )
    })
    .await
    .map_err(err)?
}

/// `direct_start`'s order: a worker this app runs already is left alone,
/// and its browser with it (a second Start does nothing); else the
/// browser first (`browser`), then the worker. When the worker doesn't
/// start, its browser is ended only if this start started it and no
/// other start of the worker runs.
fn start_with_browser(
    browser: bool,
    running: impl Fn() -> bool,
    start_browser: impl FnOnce() -> Result<browser::Started, String>,
    start_worker: impl FnOnce() -> Result<u32, String>,
    stop_browser: impl FnOnce(),
    log: &browser::Log,
) -> Result<DirectStarted, String> {
    if running() {
        return Err("this worker is already running".into());
    }
    let mut state = String::new();
    let mut launched = false;
    if browser {
        match start_browser() {
            Ok(s) => {
                state = "starting".into();
                launched = s.launched;
            }
            Err(e) => {
                log(format!("browser: {e}"));
                state = e;
            }
        }
    }
    match start_worker() {
        Ok(pid) => Ok(DirectStarted {
            pid,
            browser: state,
        }),
        Err(e) => {
            if launched && !running() {
                stop_browser();
            }
            Err(e)
        }
    }
}

/// Lines for the app log of one worker, from any thread.
fn app_logger<R: Runtime>(app: &AppHandle<R>, home: &str) -> browser::Log {
    let app = app.clone();
    let home = home.to_string();
    std::sync::Arc::new(move |line: String| {
        let _ = app.emit(
            "app-log",
            LogLine {
                home: home.clone(),
                line,
            },
        );
    })
}

/// What the Set up card says about the worker's browser: the browser,
/// Python, Playwright in the app's environment (browser.rs).
#[tauri::command]
async fn browser_check() -> browser::Check {
    tauri::async_runtime::spawn_blocking(browser::check)
        .await
        .unwrap_or_default()
}

/// "Install Playwright": the app's environment made with the owner's
/// Python, then pip, the output in the app log, ending with an
/// `install-done` event (id "playwright"), as `runtime_install` does.
#[tauri::command]
async fn browser_install<R: Runtime>(app: AppHandle<R>, home: String) -> Result<(), String> {
    let log = app_logger(&app, &home);
    let steps = tauri::async_runtime::spawn_blocking(|| {
        let os = platform::os();
        let dir = browser::env_dir();
        let works = browser::env_works(&browser::env_python_in(&dir, os));
        let python = if works {
            None
        } else {
            browser::find_python().0
        };
        browser::install_steps(python.as_ref(), &dir, os, works).map(|s| (s, dir))
    })
    .await
    .map_err(err)??;
    std::thread::spawn(move || {
        let (steps, dir) = steps;
        if let Some(parent) = dir.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut status = 0;
        for argv in steps {
            log(format!("playwright: {}", argv.join(" ")));
            status = engine::run_streaming(&argv, &std::env::temp_dir(), &HashMap::new(), |line| {
                log(line)
            });
            if status != 0 {
                break;
            }
        }
        let _ = app.emit(
            "install-done",
            InstallDone {
                id: "playwright".into(),
                status,
            },
        );
    });
    Ok(())
}

/// "Show browser": the worker's browser to the front, or started again
/// if the owner closed it (browser.rs).
#[tauri::command]
async fn browser_show<R: Runtime>(
    app: AppHandle<R>,
    home: String,
    name: String,
) -> Result<String, String> {
    let log = app_logger(&app, &home);
    tauri::async_runtime::spawn_blocking(move || browser::show(Path::new(&home), &name, log))
        .await
        .map_err(err)?
}

// --- a Mac: the browser of a worker in a container (browser.rs's mac) ---

/// What Set up and Settings offer for a worker in a container: the
/// browsers it can run on the Mac, and why not when it can't.
#[tauri::command]
async fn mac_browsers() -> serde_json::Value {
    tauri::async_runtime::spawn_blocking(mac_choices)
        .await
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn mac_choices() -> serde_json::Value {
    serde_json::to_value(browser::mac::choices()).unwrap_or_default()
}

#[cfg(not(target_os = "macos"))]
fn mac_choices() -> serde_json::Value {
    serde_json::json!({"installed": [], "why": "not a Mac"})
}

/// The browser on the Mac of a worker in a container, started unless it
/// runs (the window asks while the worker starts and runs, as myous for
/// Mac does). `browser_app` is app.json's (a bundle id, "" the first).
#[tauri::command]
async fn mac_browser_ensure<R: Runtime>(
    app: AppHandle<R>,
    home: String,
    name: String,
    browser_app: String,
    hidden: bool,
) -> Result<String, String> {
    let log = app_logger(&app, &home);
    tauri::async_runtime::spawn_blocking(move || {
        mac_ensure(&home, &name, &browser_app, hidden, log)
    })
    .await
    .map_err(err)?
}

#[cfg(target_os = "macos")]
fn mac_ensure(
    home: &str,
    name: &str,
    browser_app: &str,
    hidden: bool,
    log: browser::Log,
) -> Result<String, String> {
    browser::mac::ensure(Path::new(home), name, browser_app, hidden, log)
}

#[cfg(not(target_os = "macos"))]
fn mac_ensure(_: &str, _: &str, _: &str, _: bool, _: browser::Log) -> Result<String, String> {
    Err("only on a Mac".into())
}

/// Show browser for a worker in a container on a Mac: out of hiding and
/// to the front, or started.
#[tauri::command]
async fn mac_browser_show<R: Runtime>(
    app: AppHandle<R>,
    home: String,
    name: String,
    browser_app: String,
) -> Result<String, String> {
    let log = app_logger(&app, &home);
    tauri::async_runtime::spawn_blocking(move || mac_show(&home, &name, &browser_app, log))
        .await
        .map_err(err)?
}

#[cfg(target_os = "macos")]
fn mac_show(
    home: &str,
    name: &str,
    browser_app: &str,
    log: browser::Log,
) -> Result<String, String> {
    browser::mac::show_container(Path::new(home), name, browser_app, log)
}

#[cfg(not(target_os = "macos"))]
fn mac_show(_: &str, _: &str, _: &str, _: browser::Log) -> Result<String, String> {
    Err("only on a Mac".into())
}

/// Settings' "Start this worker's browser hidden", at once.
#[tauri::command]
fn mac_browser_hidden(home: String, hidden: bool) {
    #[cfg(target_os = "macos")]
    browser::mac::set_hidden(Path::new(&home), hidden);
    #[cfg(not(target_os = "macos"))]
    let _ = (home, hidden);
}

/// Ends the worker's browser (a Mac's container worker stopped, or its
/// browser moved into the container).
#[tauri::command]
async fn browser_stop(home: String) {
    let _ = tauri::async_runtime::spawn_blocking(move || browser::stop(Path::new(&home))).await;
}

/// Stops a worker that runs without a container: the one this app
/// started, else the process `pid` from its worker.json while that pid is
/// still the worker. True if it was this app's: its exit then arrives as
/// a `direct-exit` event; else a `stop` event once the stop is over (it
/// exited, was killed, or there was nothing to stop).
#[tauri::command]
async fn direct_stop<R: Runtime>(app: AppHandle<R>, home: String, pid: u32) -> bool {
    let log = app_logger(&app, &home);
    tauri::async_runtime::spawn_blocking(move || {
        let h = home.clone();
        let ours = direct::stop(&home, pid, log, move || {
            let _ = app.emit(
                "compose-done",
                Done {
                    home: h,
                    op: "stop".into(),
                    status: 0,
                },
            );
        });
        browser::stop(Path::new(&home));
        ours
    })
    .await
    .unwrap_or(false)
}

/// The end of a worker's worker.log (its own output without a container).
#[tauri::command]
fn worker_log(home: String) -> String {
    direct::log_tail(&home, 64 * 1024)
}

/// Writes the embedded compose file into the worker folder and returns its path.
#[tauri::command]
fn write_compose(home: String) -> Result<String, String> {
    let p = PathBuf::from(&home);
    paths::ensure(&p).map_err(err)?;
    let text = COMPOSE_TEMPLATE.replace("@VERSION@", &image_tag());
    let file = paths::compose_file(&p);
    files::write_atomic(&file, text.as_bytes()).map_err(err)?;
    Ok(file.to_string_lossy().to_string())
}

#[derive(Serialize, Clone)]
struct LogLine {
    home: String,
    line: String,
}

#[derive(Serialize, Clone)]
struct Done {
    home: String,
    op: String,
    status: i32,
}

/// Runs `docker compose -f FILE -p PROJECT ARGS…` in the worker folder on
/// a thread, streaming its output as `app-log` events and ending with a
/// `compose-done` event. `env` is added to the process environment, with
/// the owner's timezone and language.
#[tauri::command]
fn compose_run<R: Runtime>(
    app: AppHandle<R>,
    home: String,
    compose_file: String,
    project: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    op: String,
) -> Result<(), String> {
    let mut full = engine::compose_args(&compose_file, &project);
    full.extend(args);
    let mut env = env;
    env.entry("MYOUS_TZ".into())
        .or_insert_with(platform::local_zone);
    let lang = platform::language();
    if !lang.is_empty() {
        env.entry("MYOUS_LANG".into()).or_insert(lang);
    }
    let dir = PathBuf::from(&home);
    paths::ensure(&dir).map_err(err)?;
    let _ = app.emit(
        "app-log",
        LogLine {
            home: home.clone(),
            line: format!("{op}: {}", full.join(" ")),
        },
    );
    std::thread::spawn(move || {
        let h = home.clone();
        let a = app.clone();
        let status = engine::run_streaming(&full, &dir, &env, move |line| {
            let _ = a.emit(
                "app-log",
                LogLine {
                    home: h.clone(),
                    line,
                },
            );
        });
        let _ = app.emit("compose-done", Done { home, op, status });
    });
    Ok(())
}

#[tauri::command]
async fn browser_port(compose_file: String, project: String, legacy: bool) -> String {
    tauri::async_runtime::spawn_blocking(move || {
        engine::browser_port(&engine::compose_args(&compose_file, &project), legacy)
    })
    .await
    .unwrap_or_default()
}

#[tauri::command]
async fn container_logs(compose_file: String, project: String) -> String {
    tauri::async_runtime::spawn_blocking(move || {
        engine::logs(&engine::compose_args(&compose_file, &project))
    })
    .await
    .unwrap_or_default()
}

/// The worker containers, for Remove stale containers to offer.
#[tauri::command]
async fn list_containers() -> Result<Vec<engine::Container>, String> {
    tauri::async_runtime::spawn_blocking(engine::containers)
        .await
        .map_err(err)?
}

/// Removes what the owner picked: `stopped` without force, `running` with it.
#[tauri::command]
async fn remove_containers(
    stopped: Vec<engine::Container>,
    running: Vec<engine::Container>,
) -> engine::Removal {
    tauri::async_runtime::spawn_blocking(move || engine::remove(&stopped, &running))
        .await
        .unwrap_or_else(|e| engine::Removal {
            log: format!("remove containers: {e}"),
            removed: Vec::new(),
        })
}

#[tauri::command]
async fn read_agents(extra: Vec<String>) -> Vec<agents::Agent> {
    tauri::async_runtime::spawn_blocking(move || agents::read(&agents::homes(&extra)))
        .await
        .unwrap_or_default()
}

#[derive(Serialize)]
struct Outcome {
    status: i32,
    output: String,
}

/// Runs the agent's own client as that agent, for what the owner asked
/// (accepting a pairing code).
#[tauri::command]
async fn agent_run(home: String, binary: String, args: Vec<String>) -> Outcome {
    tauri::async_runtime::spawn_blocking(move || {
        let (status, output) = agents::run_sync(&home, &binary, &args, Duration::from_secs(120));
        Outcome { status, output }
    })
    .await
    .unwrap_or(Outcome {
        status: -1,
        output: "crashed".into(),
    })
}

#[tauri::command]
async fn latest_release() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(hub::latest_release)
        .await
        .map_err(err)?
}

/// A QR code for the pairing link, as a PNG data URL.
#[tauri::command]
fn qr_png(text: String) -> Result<String, String> {
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(err)?;
    let img = code
        .render::<image::Luma<u8>>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .build();
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(err)?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
    ))
}

/// The tray's icon (a PNG, base64), tooltip and menu, as the window
/// decided them.
#[tauri::command]
fn set_tray<R: Runtime>(
    app: AppHandle<R>,
    icon: Option<String>,
    tooltip: String,
    items: Vec<tray::Item>,
) -> Result<(), String> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Err("no tray".into());
    };
    if let Some(b64) = icon {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(err)?;
        let img = tauri::image::Image::from_bytes(&bytes).map_err(err)?;
        tray.set_icon(Some(img)).map_err(err)?;
    }
    tray.set_tooltip(Some(tooltip)).map_err(err)?;
    let menu = tray::build(&app, &items).map_err(err)?;
    tray.set_menu(Some(menu)).map_err(err)?;
    Ok(())
}

/// The count of new requests while the window is away, where a Mac shows
/// it: next to the menu bar icon and on the Dock icon (which shows only
/// when the Dock setting is on). 0 clears both. Elsewhere the tooltip
/// carries it, so this does nothing.
#[tauri::command]
fn set_counts<R: Runtime>(app: AppHandle<R>, fresh: u32) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            // A space first, as the Mac app's title has: it keeps the
            // number off the icon.
            let title = (fresh > 0).then(|| format!(" {fresh}"));
            tray.set_title(title).map_err(err)?;
        }
        if let Some(w) = app.get_webview_window("main") {
            w.set_badge_count((fresh > 0).then_some(i64::from(fresh)))
                .map_err(err)?;
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, fresh);
    Ok(())
}

/// The Dock setting changed (Settings): a Mac shows or hides the app's
/// Dock icon at once, as myous for Mac does. Nothing elsewhere.
#[tauri::command]
fn set_dock<R: Runtime>(app: AppHandle<R>, on: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        app.set_activation_policy(dock_policy(on)).map_err(err)?;
        // Leaving the Dock deactivates the app; the owner was just in its
        // window, so keep it in front.
        if let Some(w) = app.get_webview_window("main") {
            if w.is_visible().unwrap_or(false) {
                let _ = w.set_focus();
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, on);
    Ok(())
}

/// The Dock setting, `dock` in the first worker's app.json where myous
/// for Mac keeps it: off unless the owner turned it on.
#[cfg(target_os = "macos")]
fn dock_setting() -> bool {
    files::read_json(&paths::config_file(&paths::default_home()))
        .and_then(|v| v.get("dock").and_then(Value::as_bool))
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn dock_policy(dock: bool) -> tauri::ActivationPolicy {
    if dock {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    }
}

/// A Mac: the window's size in points, as the window measured its
/// content (myous for Mac fits its window the same way), kept on the
/// screen, its top edge where it was. `viewport` is the height the page
/// has now: Tauri sizes a Mac's window by its frame, and the page gets
/// that less the title bar, so the difference is added. Nothing
/// elsewhere: there the owner sizes the window.
#[tauri::command]
fn fit_window<R: Runtime>(
    app: AppHandle<R>,
    width: f64,
    height: f64,
    viewport: f64,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    if let Some(w) = app.get_webview_window("main") {
        let scale = w.scale_factor().map_err(err)?;
        let screen = w
            .current_monitor()
            .ok()
            .flatten()
            .or_else(|| w.primary_monitor().ok().flatten())
            .map(|m| f64::from(m.work_area().size.height) / m.scale_factor())
            .unwrap_or(800.0);
        let now = w.inner_size().map_err(err)?.to_logical::<f64>(scale).height;
        let bar = if viewport > 0.0 {
            (now - viewport).clamp(0.0, 100.0)
        } else {
            0.0
        };
        let (width, height) = fit_size(width, height + bar, screen);
        let top_left = w.outer_position().ok();
        w.set_size(tauri::LogicalSize::new(width, height))
            .map_err(err)?;
        // AppKit keeps the bottom edge; keep the top, as a Mac window grows downwards.
        if let Some(p) = top_left {
            w.set_position(p).map_err(err)?;
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, width, height, viewport);
    Ok(())
}

/// The size a Mac's window takes: what the content asks, within 400 to
/// 1000 points wide, and at least 240 high but no more than the screen
/// shows below the menu bar (less a margin for the title bar).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn fit_size(width: f64, height: f64, screen: f64) -> (f64, f64) {
    let most = (screen - 60.0).max(240.0);
    (width.clamp(400.0, 1000.0), height.clamp(240.0, most))
}

/// A Mac: the app menu's items (the window's ⋯ menu elsewhere), as the
/// window decided them.
#[tauri::command]
fn set_app_menu<R: Runtime>(app: AppHandle<R>, items: Vec<tray::Item>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    app.set_menu(tray::app_menu(&app, &items).map_err(err)?)
        .map_err(err)?;
    #[cfg(not(target_os = "macos"))]
    let _ = (app, items);
    Ok(())
}

#[tauri::command]
fn show_window<R: Runtime>(app: AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        let _ = app.emit("window", true);
    }
}

#[tauri::command]
fn window_visible<R: Runtime>(app: AppHandle<R>) -> bool {
    app.get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

/// Quit (the window's and the tray's): the window and the tray icon go
/// at once, the workers this app runs without a container and the
/// browsers are stopped off the main thread (up to a worker's ten
/// seconds), then the app exits. Cmd+Q (a Mac's app menu) ends through
/// RunEvent::Exit, which stops them too.
#[tauri::command]
async fn quit<R: Runtime>(app: AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_visible(false);
    }
    let _ = tauri::async_runtime::spawn_blocking(stop_everything).await;
    app.exit(0)
}

/// Stops every worker this app runs without a container and every
/// browser it runs, the two at once, waiting for them (so none is left
/// behind).
fn stop_everything() {
    let workers = std::thread::spawn(direct::stop_all);
    browser::stop_all();
    let _ = workers.join();
}

/// Appends to app.log in the first worker's folder (the window keeps its
/// own copy for the log view).
#[tauri::command]
fn log_line(line: String) {
    files::append_app_log(&line)
}

/// The sound an urgent notification plays, by the name each system's
/// notifications know: the default toast sound on Windows, the default
/// sound on a Mac, the sound theme's new message on Linux.
const URGENT_SOUND: &str = if cfg!(windows) {
    "Default"
} else if cfg!(target_os = "macos") {
    "NSUserNotificationDefaultSoundName"
} else {
    "message-new-instant"
};

/// A question's worker folder and request id (`ask` in `notify`).
#[derive(Deserialize)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct Ask {
    home: String,
    id: String,
}

/// Shows a notification. An urgent one (a request waiting for the owner)
/// also plays a sound and asks for attention on the window: the taskbar
/// button flashes on Windows, the Dock icon bounces on a Mac (when it has
/// one), the window is marked urgent on Linux.
///
/// On Linux and Windows it is plain, without Allow and Refuse: the window,
/// which the app brings up for a question, has them. On a Mac (the app,
/// with notifications allowed) every notification goes through
/// UserNotifications, as myous for Mac's do, and a question's (`ask`)
/// has Allow and Refuse; true then, and the window needn't come up.
/// Without permission the Mac app shows none (the window comes up for a
/// question); a program run from target/ has the plugin's plain one.
#[tauri::command]
fn notify<R: Runtime>(
    app: AppHandle<R>,
    title: String,
    body: String,
    urgent: bool,
    ask: Option<Ask>,
) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    let posted = usernotify::post(
        &title,
        &body,
        urgent,
        ask.as_ref().map(|a| (a.home.as_str(), a.id.as_str())),
    );
    #[cfg(not(target_os = "macos"))]
    let posted = false;
    // A Mac app that took UserNotifications can't post the plugin's
    // (NSUserNotification) ones as well: usernoted refuses a process that
    // mixes the two. Not allowed there means not allowed for either.
    #[cfg(target_os = "macos")]
    let plain = !posted && !usernotify::active();
    #[cfg(not(target_os = "macos"))]
    let plain = true;
    if plain {
        let mut n = app.notification().builder().title(title).body(body);
        if urgent {
            n = n.sound(URGENT_SOUND);
        }
        n.show().map_err(err)?;
    }
    if urgent {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.request_user_attention(Some(tauri::UserAttentionType::Critical));
        }
    }
    Ok(posted && ask.is_some())
}

/// A Mac: what the owner did with a notification. A click on it brings up
/// the window. Allow or Refuse on a question answers it as the window's
/// buttons do (answer_approval), in the worker whose approvals hold it
/// (myous for Mac looks it up the same way), and tells the window.
#[cfg(target_os = "macos")]
fn notification_response<R: Runtime>(app: &AppHandle<R>, response: usernotify::Response) {
    #[derive(Clone, Serialize)]
    struct Answered {
        home: String,
        id: String,
        verdict: String,
        error: String,
    }
    let (id, verdict) = match response {
        usernotify::Response::Open => return show_window(app.clone()),
        usernotify::Response::Answer { id, verdict } => (id, verdict),
    };
    let home = if files::valid_id(&id) {
        paths::all_homes()
            .into_iter()
            .find(|h| paths::approvals_dir(h).join(format!("{id}.json")).is_file())
    } else {
        None
    };
    let result = match &home {
        Some(h) => answer_approval(h.to_string_lossy().to_string(), id.clone(), verdict.into()),
        None => Err("the request no longer waits for an answer".into()),
    };
    let _ = app.emit(
        "approval-answered",
        Answered {
            home: home
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_default(),
            id,
            verdict: verdict.into(),
            error: result.err().unwrap_or_default(),
        },
    );
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://")
        || url.starts_with("http://localhost:")
        || url.starts_with("http://127.0.0.1:"))
    {
        return Err("only web links open".into());
    }
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(err)
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(err)
}

#[tauri::command]
async fn pick_folder<R: Runtime>(app: AppHandle<R>) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .blocking_pick_folder()
            .and_then(|p| p.into_path().ok())
            .map(|p| p.to_string_lossy().to_string())
    })
    .await
    .ok()
    .flatten()
}

#[tauri::command]
fn copy_text<R: Runtime>(app: AppHandle<R>, text: String) -> Result<(), String> {
    app.clipboard().write_text(text).map_err(err)
}

/// Whether the app opens at login. A Mac (the app): a login item through
/// SMAppService, as myous for Mac (loginitem.rs); elsewhere, and a Mac's
/// program run from target/, the autostart plugin.
#[tauri::command]
fn autostart_get<R: Runtime>(app: AppHandle<R>) -> bool {
    #[cfg(target_os = "macos")]
    if usernotify::in_app_bundle() {
        return loginitem::is_enabled();
    }
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn autostart_set<R: Runtime>(app: AppHandle<R>, on: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    if usernotify::in_app_bundle() {
        return loginitem::set(on);
    }
    if on {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    }
    .map_err(err)
}

#[tauri::command]
fn file_exists(path: String) -> bool {
    Path::new(&path).exists()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    files::append_app_log(&format!(
        "{} myous {} starting ({})",
        now_stamp(),
        version(),
        platform::os()
    ));
    // A Windows GUI program has no console: a crash would leave no trace.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        files::append_app_log(&format!("{} crashed: {info}", now_stamp()));
        default_hook(info);
    }));
    // Before the window exists, so the taskbar knows it by the same id as the toasts.
    toast::set_process_id();
    tauri::Builder::default()
        // First, so a second launch exits before it makes a tray icon: it
        // shows this copy's window instead.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_window(app.clone());
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        // On a Mac the app opens at login through SMAppService instead
        // (loginitem.rs), as myous for Mac; the plugin's LaunchAgent
        // (~/Library/LaunchAgents/myous.plist) is left for a program run
        // from target/, and an upgrade removes it (setup below).
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            app_info,
            read_workers,
            write_config,
            send_command,
            set_paused,
            answer_approval,
            set_review,
            new_worker,
            remove_worker,
            probe_runtime,
            launch_runtime,
            runtime_options,
            runtime_install,
            hub_check,
            write_compose,
            compose_run,
            browser_port,
            container_logs,
            list_containers,
            remove_containers,
            read_agents,
            agent_run,
            latest_release,
            qr_png,
            set_tray,
            set_counts,
            set_dock,
            fit_window,
            set_app_menu,
            show_window,
            window_visible,
            quit,
            log_line,
            notify,
            open_url,
            open_path,
            pick_folder,
            copy_text,
            autostart_get,
            autostart_set,
            file_exists,
            containers_check,
            enable_wsl,
            direct_start,
            direct_stop,
            worker_log,
            browser_check,
            browser_install,
            browser_show,
            mac_browsers,
            mac_browser_ensure,
            mac_browser_show,
            mac_browser_hidden,
            browser_stop
        ])
        .setup(|app| {
            // A menu bar app on a Mac (Info.plist's LSUIElement), with a
            // Dock icon only when the owner asked for one, as myous for Mac.
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(dock_policy(dock_setting()));
                // The menu bar until the window sends its items; and a
                // window that takes its content's size (fit_window), not
                // one the owner drags, as myous for Mac.
                app.set_menu(tray::app_menu(app.handle(), &[])?)?;
                if let Some(w) = app.get_webview_window("main") {
                    w.set_min_size(Some(tauri::LogicalSize::new(400.0, 240.0)))?;
                    w.set_resizable(false)?;
                    w.set_size(tauri::LogicalSize::new(560.0, 420.0))?;
                    w.center()?;
                }
                // Notifications with Allow and Refuse, as myous for Mac's
                // (none outside an app bundle: the plugin's plain ones).
                let handle = app.handle().clone();
                usernotify::setup(move |r| notification_response(&handle, r));
                // Open at login: from the plugin's LaunchAgent to a login item.
                if usernotify::in_app_bundle() {
                    let agent = app.autolaunch();
                    let on = agent.is_enabled().unwrap_or(false);
                    if let Some(line) = loginitem::migrate(on, || agent.disable().map_err(err)) {
                        files::append_app_log(&line);
                    }
                }
            }
            // Windows drops the toasts of an app it doesn't know.
            toast::register();
            let icon = app.default_window_icon().cloned().ok_or("no icon")?;
            let menu = tray::build(
                app.handle(),
                &[tray::Item {
                    id: "open".into(),
                    label: "Open myous…".into(),
                    enabled: true,
                    separator: false,
                    accelerator: String::new(),
                    children: vec![],
                }],
            )?;
            tauri::tray::TrayIconBuilder::with_id(TRAY_ID)
                .icon(icon)
                .tooltip("myous")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| {
                    let id = event.id().0.clone();
                    if id == "open" {
                        show_window(app.clone());
                    }
                    let _ = app.emit("tray", id);
                })
                .build(app)?;
            // If the window's script never starts (a broken web view), show
            // the window anyway, so the owner sees something and the log
            // says why, rather than a tray icon and nothing else.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(10));
                if !PAGE_READY.load(std::sync::atomic::Ordering::SeqCst) {
                    files::append_app_log(
                        "the window's script didn't start within 10 s; showing the window anyway",
                    );
                    let h = handle.clone();
                    let _ = handle.run_on_main_thread(move || show_window(h));
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                let _ = window.app_handle().emit("window", false);
            }
        })
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            // Most likely on Windows: no WebView2 runtime. Say so where the owner can find it.
            files::append_app_log(&format!("{} could not start: {e}", now_stamp()));
            std::process::exit(1);
        })
        .run(|app, event| {
            // Workers without a container are this app's children: don't leave them orphaned.
            if let tauri::RunEvent::Exit = event {
                stop_everything();
            }
            // A Mac: a click on the Dock icon (when the setting shows one), or
            // opening the app again from the Finder, shows the window, as
            // myous for Mac does.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show_window(app.clone());
            }
            #[cfg(not(target_os = "macos"))]
            let _ = app;
        });
}

fn now_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded compose file is the Mac app's (comments aside), so
    /// the three apps run one worker.
    #[test]
    fn compose_matches_the_mac_app() {
        let mac = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../mac/compose-image.yml"
        ))
        .unwrap();
        let strip = |s: &str| {
            s.lines()
                .filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(strip(COMPOSE_TEMPLATE), strip(&mac));
        assert!(COMPOSE_TEMPLATE.contains("@VERSION@"));
    }

    /// A Mac's window: what the content asks, kept on the screen.
    #[test]
    fn fit_size_keeps_the_window_on_the_screen() {
        assert_eq!(fit_size(560.0, 511.0, 900.0), (560.0, 511.0));
        assert_eq!(fit_size(560.0, 1400.0, 900.0), (560.0, 840.0));
        assert_eq!(fit_size(560.0, 100.0, 900.0), (560.0, 240.0));
        assert_eq!(fit_size(5000.0, 300.0, 100.0), (1000.0, 240.0));
    }

    /// The default worker name: one line, never empty (a Mac's Computer
    /// Name, else the short host name).
    #[test]
    fn host_name_is_one_line() {
        let h = platform::hostname();
        assert!(!h.is_empty() && !h.contains('\n'), "{h:?}");
    }

    /// A second Start while the worker runs leaves it and its browser
    /// alone; a failed start ends only a browser it started itself.
    #[test]
    fn start_leaves_a_running_worker_and_its_browser_alone() {
        use std::cell::{Cell, RefCell};
        let log: browser::Log = std::sync::Arc::new(|_| {});
        let started = |launched| browser::Started {
            port: 9,
            app: "Edge".into(),
            python: String::new(),
            launched,
        };
        let calls = RefCell::new(Vec::<&str>::new());
        let call = |what| calls.borrow_mut().push(what);
        // Running already: nothing touched.
        let r = start_with_browser(
            true,
            || true,
            || {
                call("browser");
                Ok(started(true))
            },
            || {
                call("worker");
                Ok(1)
            },
            || call("stop browser"),
            &log,
        );
        assert!(r.unwrap_err().contains("already running"));
        assert!(calls.borrow().is_empty());
        // Started: the browser first.
        let r = start_with_browser(
            true,
            || false,
            || {
                call("browser");
                Ok(started(true))
            },
            || {
                call("worker");
                Ok(42)
            },
            || call("stop browser"),
            &log,
        )
        .unwrap();
        assert_eq!((r.pid, r.browser.as_str()), (42, "starting"));
        assert_eq!(*calls.borrow(), ["browser", "worker"]);
        // The worker didn't start: a browser this start made goes; one
        // that ran already, or one another start of the worker uses, stays.
        for (launched, other_start_runs, stopped) in [
            (true, false, true),
            (false, false, false),
            (true, true, false),
        ] {
            calls.borrow_mut().clear();
            let asked = Cell::new(0);
            let r = start_with_browser(
                true,
                || {
                    asked.set(asked.get() + 1);
                    other_start_runs && asked.get() > 1
                },
                || Ok(started(launched)),
                || Err("could not start".into()),
                || call("stop browser"),
                &log,
            );
            assert_eq!(r.unwrap_err(), "could not start");
            assert_eq!(calls.borrow().contains(&"stop browser"), stopped);
        }
        // No browser asked for: none started, none stopped.
        calls.borrow_mut().clear();
        let r = start_with_browser(
            false,
            || false,
            || {
                call("browser");
                Ok(started(true))
            },
            || Err("no".into()),
            || call("stop browser"),
            &log,
        );
        assert!(r.is_err() && calls.borrow().is_empty());
    }

    #[test]
    fn qr_is_a_png() {
        let url = qr_png("https://myoushq.com/p/4821#K7F3QX".into()).unwrap();
        assert!(url.starts_with("data:image/png;base64,iVBOR"));
    }
}
