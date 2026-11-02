fn main() {
    let args = std::env::args();
    if let Err(err) = knr_cli::run(args) {
        if matches!(err, knr_cli::CliError::Args(_)) {
            eprintln!("{err}");
        }
        std::process::exit(1);
    }
}
