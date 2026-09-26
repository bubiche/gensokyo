use std::io::Read;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("gensokyo {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        // `--foreground` is what launchd passes; the daemon never forks either way.
        Some("daemon") => gensokyo::daemon::server::main(),
        // Claude Code runs these for every resident. Whatever happens they exit 0: a hook's exit
        // 2 would block the prompt or the stop it was called for.
        Some("_hook" | "_statusline") => {
            let _ = std::io::stdin().take(1 << 20).read_to_end(&mut Vec::new());
            ExitCode::SUCCESS
        }
        Some(_) => gensokyo::cli::main(&args),
        None => {
            eprintln!("gensokyo {}: the client is not implemented yet", env!("CARGO_PKG_VERSION"));
            ExitCode::from(2)
        }
    }
}
