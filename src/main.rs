// Hide the console window in release builds; debug builds keep it so
// `cargo run` output (panics, logs) stays visible in the terminal.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    match tokenmonitor::cli::parse() {
        tokenmonitor::cli::Command::Help => {
            tokenmonitor::cli::print_help("tokenmonitor-app", &[]);
            Ok(())
        }
        tokenmonitor::cli::Command::Version => {
            tokenmonitor::cli::print_version("tokenmonitor-app");
            Ok(())
        }
        tokenmonitor::cli::Command::CheckUpdate => {
            eprintln!(
                "桌面版请打开「设置 → 更新检查」；命令行检查请用 tokenmonitor-tui --check-update"
            );
            std::process::exit(2);
        }
        tokenmonitor::cli::Command::Unknown(arg) => tokenmonitor::cli::exit_usage_error(&arg),
        tokenmonitor::cli::Command::Run => tokenmonitor::app::run(),
    }
}
