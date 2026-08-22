// Thin entry point for the terminal frontend. Keep the console window on all
// builds so the TUI (and its output) stays visible in the terminal.

fn main() -> anyhow::Result<()> {
    tokenmonitor::tui::run()
}
