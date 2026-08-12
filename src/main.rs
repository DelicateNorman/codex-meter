fn main() {
    if let Err(error) = codex_meter::cli::run() {
        eprintln!("error: {error:#}");
        std::process::exit(codex_meter::cli::error_exit_code(&error));
    }
}
