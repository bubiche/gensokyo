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
        Some(v @ ("_hook" | "_statusline")) => {
            std::panic::set_hook(Box::new(|_| {}));
            let _ = match v {
                "_hook" => std::panic::catch_unwind(gensokyo::hooks::hook_main),
                _ => std::panic::catch_unwind(|| {
                    gensokyo::hooks::statusline_main(args.get(1).map_or("", String::as_str))
                }),
            };
            ExitCode::SUCCESS
        }
        Some(_) => gensokyo::cli::main(&args),
        None => gensokyo::client::app::main(),
    }
}
