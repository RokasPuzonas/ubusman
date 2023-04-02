#[cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use app::App;
use tokio::runtime::Runtime;

mod app;
mod ubus;
mod async_line_reader;
mod syntax_highlighting;

// TODO: Save config to file
// TODO: call history

fn main() {
    let rt = Runtime::new().expect("Unable to create Runtime");

    let _enter = rt.enter();

    let mut app = App::default();

    eframe::run_native(
        "ubusman",
        eframe::NativeOptions::default(),
        Box::new(move |cc| {
            app.init(cc);
            Box::new(app)
        })
    ).expect("Unable to create window");
}

