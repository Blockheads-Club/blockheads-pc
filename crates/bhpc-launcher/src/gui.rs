//! egui front end: draws [`Wizard`] and forwards input. No logic lives here (not part of the coverage gate).

use bhpc_launcher::import::Existing;
use bhpc_launcher::keys::Action;
use bhpc_launcher::wizard::{Step, Wizard};
use bhpc_launcher::LaunchConfig;
use eframe::egui;
use std::path::PathBuf;

struct App {
    wizard: Wizard,
    apk_text: String,
    data_text: String,
    import_text: String,
    overwrite: bool,
    key_text: Vec<String>,
    status: String,
}

pub fn run() -> eframe::Result<()> {
    let path = LaunchConfig::default_path().unwrap_or_else(|| PathBuf::from("launcher.toml"));
    let existing = LaunchConfig::load(&path).ok().flatten();
    let data = LaunchConfig::default_data_dir().unwrap_or_else(|_| PathBuf::from("blockheads-data"));
    let wizard = Wizard::new(existing, path, data);
    let app = App {
        apk_text: wizard.config.apk.display().to_string(),
        data_text: wizard.config.data_dir.display().to_string(),
        import_text: String::new(),
        overwrite: false,
        key_text: Action::ALL.iter().map(|a| wizard.config.keys.get(*a).to_string()).collect(),
        status: String::new(),
        wizard,
    };
    eframe::run_native("Blockheads PC launcher", eframe::NativeOptions::default(), Box::new(|_| Ok(Box::new(app))))
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        ctx.set_pixels_per_point(self.wizard.config.window.ui_scale);
        let dropped = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()));
        egui::CentralPanel::default().show(ctx, |ui| {
            match self.wizard.step {
                Step::Apk => self.apk_page(ui, dropped),
                Step::DataDir => self.data_page(ui),
                Step::Keys => self.keys_page(ui),
                Step::Window => self.window_page(ui),
                Step::Import => self.import_page(ui, dropped),
                Step::Done => self.done_page(ui),
            }
            if let Some(e) = &self.wizard.error {
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            }
            ui.horizontal(|ui| {
                if ui.button("Back").clicked() {
                    self.wizard.back();
                }
                if ui.add_enabled(self.wizard.can_advance(), egui::Button::new("Next")).clicked() {
                    self.wizard.advance();
                }
            });
            ui.label(&self.status);
        });
    }
}

impl App {
    fn apk_page(&mut self, ui: &mut egui::Ui, dropped: Option<PathBuf>) {
        ui.heading("Your Blockheads APK");
        ui.label("Drop the APK here or type its path. Your own copy is required; nothing is downloaded.");
        if let Some(p) = dropped {
            self.apk_text = p.display().to_string();
        }
        ui.text_edit_singleline(&mut self.apk_text);
        ui.checkbox(&mut self.wizard.config.force, "Allow unknown builds");
        if ui.button("Check").clicked() {
            self.wizard.pick_apk(&PathBuf::from(&self.apk_text));
        }
        if let Some(i) = &self.wizard.apk_info {
            ui.label(i.describe());
        }
    }

    fn data_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Data directory");
        ui.text_edit_singleline(&mut self.data_text);
        self.wizard.set_data_dir(PathBuf::from(&self.data_text));
    }

    fn keys_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Controls");
        ui.label("Examples: Left, Right, Wheel, Ctrl+Left, Esc, F11, Q");
        for (i, a) in Action::ALL.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(a.label());
                if ui.text_edit_singleline(&mut self.key_text[i]).lost_focus()
                    && !self.wizard.rebind(*a, &self.key_text[i])
                {
                    self.key_text[i] = self.wizard.config.keys.get(*a).to_string();
                }
            });
        }
    }

    fn window_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Window and graphics");
        let w = &mut self.wizard.config.window;
        ui.add(egui::DragValue::new(&mut w.width).prefix("width "));
        ui.add(egui::DragValue::new(&mut w.height).prefix("height "));
        ui.checkbox(&mut w.fullscreen, "Fullscreen");
        ui.checkbox(&mut w.vsync, "VSync");
        ui.add(egui::Slider::new(&mut w.ui_scale, 0.5..=4.0).text("UI scale"));
    }

    fn import_page(&mut self, ui: &mut egui::Ui, dropped: Option<PathBuf>) {
        ui.heading("Import saves from Android (optional)");
        ui.label("Drop a folder or zip pulled from /data/data/com.noodlecake.blockheads, or type its path.");
        if let Some(p) = dropped {
            self.import_text = p.display().to_string();
        }
        ui.text_edit_singleline(&mut self.import_text);
        ui.checkbox(&mut self.overwrite, "Overwrite existing files");
        if ui.button("Import").clicked() {
            let policy = if self.overwrite { Existing::Overwrite } else { Existing::Skip };
            self.wizard.import(&PathBuf::from(&self.import_text), policy);
        }
        if let Some(r) = &self.wizard.import_report {
            ui.label(format!("{} written, {} kept, {} ignored", r.written.len(), r.skipped_existing, r.ignored));
        }
    }

    fn done_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Ready");
        if ui.button("Save and launch").clicked() {
            if let Some(cfg) = self.wizard.finish() {
                let run = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("bhpc-run")));
                self.status = match run.map(|r| std::process::Command::new(r).args(cfg.run_args()).spawn()) {
                    Some(Ok(_)) => "Launched.".into(),
                    _ => "Saved. Start bhpc-run with the saved settings.".into(),
                };
            }
        }
    }
}
