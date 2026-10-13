// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args().any(|a| a == "--version") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return;
    }
    // The app takes no commands. A command (`myous worker`, `myous status`)
    // was meant for the myous client, which has the same name: run as the
    // app, each such call would open another copy of it.
    if let Some(cmd) = std::env::args().skip(1).find(|a| !a.starts_with('-')) {
        eprintln!("this is the myous app, not the myous client; it has no command {cmd:?}");
        std::process::exit(2);
    }
    myous_desktop_lib::run()
}
