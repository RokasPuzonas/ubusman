use eframe::CreationContext;


#[derive(Default)]
pub struct App;

impl App {
    pub fn init(&mut self, _cc: &CreationContext) {
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        use egui::*;

        egui::CentralPanel::default()
            .show(ctx, |ui| {
                ui.label("Hello World!");
        });
    }
}