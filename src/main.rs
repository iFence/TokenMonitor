// Hide the console window in release builds; debug builds keep it so
// `cargo run` output (panics, logs) stays visible in the terminal.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    tokenmonitor::app::run()
}
