pub mod args;
pub mod c_runtime;
pub mod commands;
pub mod error;
pub mod platform;

pub use args::{parse_args, BuildTarget, CliCommand};
pub use error::CliError;

pub fn run<I: IntoIterator<Item = String>>(args: I) -> Result<(), CliError> {
    let command = parse_args(args)?;
    match command {
        CliCommand::Help | CliCommand::Version => Ok(()),
        CliCommand::Check { file, json } => commands::check::execute(&file, json),
        CliCommand::Run { file, gas, trace } => commands::run::execute(&file, gas, trace),
        CliCommand::Build {
            file,
            target,
            output,
        } => commands::build::execute(&file, target, &output),
        CliCommand::Asm { file, output } => commands::asm::execute(&file, &output),
        CliCommand::Disasm { file, output } => {
            commands::disasm::execute(&file, output.as_deref())
        }
    }
}
