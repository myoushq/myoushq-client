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
mod paths;
mod platform;
mod toast;
mod tray;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use serde::Serialize;
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
    paths::all_homes()
        .iter()
        .map(|h| files::read_worker(h))
        .collect()
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
    let safe: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if safe.is_empty() {
        return Err("empty command".into());
    }
    paths::send_command(Path::new(&home), &safe).map_err(err)
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
    files::answer_approval(Path::new(&home), &id, &verdict).map_err(err)
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

#[derive(Serialize)]
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
    let mut browser_state = String::new();
    if browser {
        let (h, n, l) = (home.clone(), name.clone(), log.clone());
        let r = tauri::async_runtime::spawn_blocking(move || {
            browser::start(Path::new(&h), &n, Vec::new(), l)
        })
        .await
        .map_err(err)?;
        match r {
            Ok(_) => browser_state = "starting".into(),
            Err(e) => {
                log(format!("browser: {e}"));
                browser_state = e;
            }
        }
    }
    // No --note for the browser: the worker reads browser.json at each help.
    let args = direct::worker_args(Path::new(&home), &bin, &name, &description, &[]);
    log(format!("start: {} (output in worker.log)", args.join(" ")));
    let h = home.clone();
    let a = app.clone();
    let started = direct::start(&home, &args, move |status| {
        // The browser lives and dies with the worker.
        browser::stop(Path::new(&h));
        let _ = a.emit(
            "compose-done",
            Done {
                home: h,
                op: "direct-exit".into(),
                status,
            },
        );
    });
    match started {
        Ok(pid) => Ok(DirectStarted {
            pid,
            browser: browser_state,
        }),
        Err(e) => {
            let h = home.clone();
            let _ =
                tauri::async_runtime::spawn_blocking(move || browser::stop(Path::new(&h))).await;
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

/// Stops a worker that runs without a container: the one this app
/// started, else the process `pid` from its worker.json. True if it was
/// this app's (its exit then arrives as an event).
#[tauri::command]
async fn direct_stop(home: String, pid: u32) -> bool {
    tauri::async_runtime::spawn_blocking(move || {
        let ours = direct::stop(&home, pid);
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
    std::fs::write(&file, text).map_err(err)?;
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

#[tauri::command]
async fn remove_stale_containers() -> String {
    tauri::async_runtime::spawn_blocking(engine::remove_all)
        .await
        .unwrap_or_default()
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

#[tauri::command]
fn quit<R: Runtime>(app: AppHandle<R>) {
    direct::stop_all();
    browser::stop_all();
    app.exit(0)
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

/// Shows a notification. An urgent one (a request waiting for the owner)
/// also plays a sound and asks for attention on the window: the taskbar
/// button flashes on Windows, the Dock icon bounces on a Mac, the window
/// is marked urgent on Linux.
#[tauri::command]
fn notify<R: Runtime>(
    app: AppHandle<R>,
    title: String,
    body: String,
    urgent: bool,
) -> Result<(), String> {
    let mut n = app.notification().builder().title(title).body(body);
    if urgent {
        n = n.sound(URGENT_SOUND);
    }
    n.show().map_err(err)?;
    if urgent {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.request_user_attention(Some(tauri::UserAttentionType::Critical));
        }
    }
    Ok(())
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

#[tauri::command]
fn autostart_get<R: Runtime>(app: AppHandle<R>) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn autostart_set<R: Runtime>(app: AppHandle<R>, on: bool) -> Result<(), String> {
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
            remove_stale_containers,
            read_agents,
            agent_run,
            latest_release,
            qr_png,
            set_tray,
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
            browser_show
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
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
        .run(|_, event| {
            // Workers without a container are this app's children: don't leave them orphaned.
            if let tauri::RunEvent::Exit = event {
                direct::stop_all();
                browser::stop_all();
            }
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

    #[test]
    fn qr_is_a_png() {
        let url = qr_png("https://myoushq.com/p/4821#K7F3QX".into()).unwrap();
        assert!(url.starts_with("data:image/png;base64,iVBOR"));
    }
}
