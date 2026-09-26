fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => println!("gensokyo {}", env!("CARGO_PKG_VERSION")),
        _ => {
            eprintln!("gensokyo {}: not implemented yet", env!("CARGO_PKG_VERSION"));
            std::process::exit(2);
        }
    }
}
