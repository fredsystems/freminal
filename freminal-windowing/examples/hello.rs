//! Minimal example: opens a window with an egui label.

use freminal_windowing::{App, WindowConfig, WindowId};

struct HelloApp;

impl App for HelloApp {
    fn update(&mut self, _window_id: WindowId, ctx: &egui::Context, _gl: &glow::Context) {
        #[expect(
            deprecated,
            reason = "show_inside needs &mut Ui; top-level show takes &Context"
        )]
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Hello from freminal-windowing");
            ui.label("This is a minimal example using winit + glutin + egui.");
        });
    }

    fn on_window_created(&mut self, _window_id: WindowId, _ctx: &egui::Context) {}

    fn on_close_requested(&mut self, _window_id: WindowId) -> bool {
        true
    }

    fn clear_color(&self, _window_id: WindowId) -> [f32; 4] {
        [0.1, 0.1, 0.1, 1.0]
    }
}

fn main() {
    let config = WindowConfig {
        title: "Hello freminal-windowing".to_owned(),
        inner_size: Some((800, 600)),
        transparent: false,
        icon: None,
        app_id: Some("freminal-hello".to_owned()),
    };

    if let Err(e) = freminal_windowing::run(config, HelloApp) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
