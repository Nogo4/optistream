// Release builds are a pure window application (no console).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod jobs;
mod render;
mod theme;

/// The window has no console: panics are written next to the saved settings instead.
fn install_crash_log() {
    let Some(dir) = std::env::var_os("APPDATA").map(|d| std::path::PathBuf::from(d).join("OptiStream")) else { return };
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current().name().unwrap_or("?").to_owned();
        let trace = std::backtrace::Backtrace::force_capture();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("crash.log")) {
            use std::io::Write;
            let _ = writeln!(f, "--- {:?} thread '{thread}': {info}\n{trace}", std::time::SystemTime::now());
        }
        default(info);
    }));
}

fn main() -> eframe::Result {
    install_crash_log();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("OptiStream")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native("OptiStream", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
