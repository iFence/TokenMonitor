// Thin entry point for the terminal frontend. Keep the console window on all
// builds so the TUI (and its output) stays visible in the terminal.

fn main() -> anyhow::Result<()> {
    match tokenmonitor::cli::parse() {
        tokenmonitor::cli::Command::Help => {
            tokenmonitor::cli::print_help(
                "tokenmonitor-tui",
                &["  -u, --check-update   检查更新并退出（退出码：0 无更新 · 2 有更新 · 1 出错）"],
            );
            Ok(())
        }
        tokenmonitor::cli::Command::Version => {
            tokenmonitor::cli::print_version("tokenmonitor-tui");
            Ok(())
        }
        tokenmonitor::cli::Command::CheckUpdate => tokenmonitor::tui::check_update_cli(),
        tokenmonitor::cli::Command::Unknown(arg) => tokenmonitor::cli::exit_usage_error(&arg),
        tokenmonitor::cli::Command::Run => tokenmonitor::tui::run(),
    }
}
