#[cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release
use app::App;

mod app;

fn main() -> eframe::Result<()> {
    let mut native_options = eframe::NativeOptions::default();
    native_options.decorated = true;
    native_options.resizable = true;
    let mut app = App::default();

    eframe::run_native(
        "ubusman",
        native_options,
        Box::new(move |cc| {
            app.init(cc);
            Box::new(app)
        })
    )
}

