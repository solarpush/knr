#[cfg(target_os = "linux")]
#[global_allocator]
static KNR_ALLOC: knr_codegen::MmapPageAllocator = knr_codegen::MmapPageAllocator;

fn main() {
    let args = std::env::args();
    if let Err(err) = knr_cli::run(args) {
        if matches!(err, knr_cli::CliError::Args(_) | knr_cli::CliError::Other(_)) {
            eprintln!("{err}");
        }
        std::process::exit(1);
    }
}
