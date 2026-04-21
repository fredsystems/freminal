//! Minimal example: opens a window with an egui label.

use freminal_windowing::{App, WindowConfig, WindowHandle, WindowId};

struct HelloApp;

impl App for HelloApp {
    fn update(
        &mut self,
        _window_id: WindowId,
        ctx: &egui::Context,
        _gl: &glow::Context,
        _handle: &WindowHandle<'_>,
    ) {
        let mut root_ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::new("hello_root"),
            egui::UiBuilder::default(),
        );
        egui::CentralPanel::default().show_inside(&mut root_ui, |ui| {
            ui.heading("Hello from freminal-windowing");
            ui.label("This is a minimal example using winit + glutin + egui.");
        });
    }

    fn on_window_created(
        &mut self,
        _window_id: WindowId,
        _ctx: &egui::Context,
        _handle: &WindowHandle<'_>,
        _inner_size: (u32, u32),
    ) {
    }

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
        position: None,
        transparent: false,
        icon: None,
        app_id: Some("freminal-hello".to_owned()),
    };

    if let Err(e) = freminal_windowing::run(config, HelloApp) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
