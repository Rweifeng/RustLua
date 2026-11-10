// Windows: this is a GUI app; do not spawn a console window when launched directly.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    game::run_desktop()
}
