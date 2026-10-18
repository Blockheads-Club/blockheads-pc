//! Thin entry point of the egui launcher (built with `--features gui`; not part of the coverage gate).
mod gui;

fn main() -> eframe::Result<()> {
    gui::run()
}
