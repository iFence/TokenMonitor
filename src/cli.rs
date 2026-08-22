//! Minimal CLI argument parsing shared by both binaries: `--help`, `--version`,
//! and — terminal frontend only — `--check-update`. Hand-rolled on purpose:
//! three flags do not justify a `clap` dependency.

use std::env;

/// What the user asked for on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `-h` / `--help`: print usage and exit.
    Help,
    /// `-V` / `--version`: print version and exit.
    Version,
    /// `-u` / `--check-update` (terminal frontend only): check for a newer
    /// release and exit.
    CheckUpdate,
    /// An unrecognized `-x` flag.
    Unknown(String),
    /// No flags: launch the frontend.
    Run,
}

/// Parse `std::env::args()`. `--help` anywhere wins; otherwise the first
/// recognized flag decides; unknown flags are reported; plain arguments are
/// ignored.
pub fn parse() -> Command {
    parse_args(env::args().skip(1))
}

fn parse_args(args: impl Iterator<Item = String>) -> Command {
    let mut first: Option<Command> = None;
    for arg in args {
        let cmd = match arg.as_str() {
            "-h" | "--help" => Command::Help,
            "-V" | "--version" => Command::Version,
            "-u" | "--check-update" => Command::CheckUpdate,
            _ if arg.starts_with('-') && arg.len() > 1 => Command::Unknown(arg),
            _ => continue,
        };
        if cmd == Command::Help {
            return Command::Help;
        }
        if first.is_none() {
            first = Some(cmd);
        }
    }
    first.unwrap_or(Command::Run)
}

/// Print `--version` output.
pub fn print_version(bin: &str) {
    println!("{bin} {}", env!("CARGO_PKG_VERSION"));
}

/// Print `--help` output. `extra` may carry additional flag lines (already
/// indented) that only apply to one binary.
pub fn print_help(bin: &str, extra: &[&str]) {
    println!(
        "{bin} {} — AI 编程工具 Token 用量追踪",
        env!("CARGO_PKG_VERSION")
    );
    println!();
    println!("用法: {bin} [选项]");
    println!();
    println!("选项:");
    println!("  -h, --help            显示本帮助并退出");
    println!("  -V, --version         显示版本并退出");
    for line in extra {
        println!("{line}");
    }
}

/// Report an unrecognized flag on stderr and exit with status 2.
pub fn exit_usage_error(arg: &str) -> ! {
    eprintln!("未知选项: {arg}");
    eprintln!("用 --help 查看用法。");
    std::process::exit(2);
}
