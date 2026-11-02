pub mod args;
pub mod c_runtime;
pub mod commands;
pub mod error;
pub mod platform;

pub use args::{
    parse_args, BuildTarget, CheckArgs, CliCommand, Command, ExploreArgs, ExploreFormat, SkillArgs,
};
pub use error::CliError;

pub fn run<I: IntoIterator<Item = String>>(args: I) -> Result<(), CliError> {
    let command = parse_args(args)?;
    match command {
        CliCommand::Help | CliCommand::Version => Ok(()),
        CliCommand::Check(args) => commands::check::execute(&args.file, args.json, args.graph),
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
        CliCommand::Update {
            check,
            force,
            version,
        } => commands::update::execute(check, force, version.as_deref()),
        CliCommand::Explore(args) => commands::explore::execute(&args),
        CliCommand::Skill(args) => commands::skill::execute(&args),
    }
}
