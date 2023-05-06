#[cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use app::App;
use eframe::IconData;
use egui::Vec2;
use tokio::runtime::Runtime;

mod app;
mod ubus;
mod async_line_reader;
mod syntax_highlighting;

// TODO: Save config to file
// TODO: call history

pub fn load_icon_from_memory(image_data: &[u8]) -> Result<IconData, image::ImageError> {
    let image = image::load_from_memory(image_data)?;
    Ok(IconData {
        rgba: image.to_rgba8().to_vec(),
        width: image.width(),
        height: image.height(),
    })
}

fn main() {
    let rt = Runtime::new().expect("Unable to create Runtime");

    let _enter = rt.enter();

    let mut app = App::default();
    let mut options = eframe::NativeOptions::default();
    options.min_window_size = Some(Vec2::new(920.0, 500.0));
    let icon_data = load_icon_from_memory(include_bytes!("./icon.png")).expect("Failed to load icon data");
    options.icon_data = Some(icon_data);
    options.vsync = true;

    eframe::run_native(
        "ubusman",
        options,
        Box::new(move |cc| {
            app.init(cc);
            Box::new(app)
        })
    ).expect("Unable to create window");
}

