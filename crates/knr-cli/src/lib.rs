pub mod args;
pub mod c_runtime;
pub mod commands;
pub mod error;
pub mod platform;

pub use args::{
    parse_args, BuildTarget, CheckArgs, CliCommand, Command, DevArgs, ExploreArgs, ExploreFormat,
    InitArgs, NewArgs, RunArgs, SkillArgs, TestArgs,
};
pub use error::CliError;

pub fn run<I: IntoIterator<Item = String>>(args: I) -> Result<(), CliError> {
    let command = parse_args(args)?;
    match command {
        CliCommand::Help | CliCommand::Version => Ok(()),
        CliCommand::Check(args) => commands::check::execute(&args.file, args.json, args.graph),
        CliCommand::Run(args) => {
            if args.watch {
                commands::dev::execute(
                    Some(&args.file),
                    150,
                    200,
                    args.gas,
                    args.trace,
                    true,
                    args.once,
                )
            } else {
                commands::run::execute(&args.file, args.gas, args.trace, args.gui)
            }
        }
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
        CliCommand::Dev(args) => commands::dev::execute(
            args.path.as_deref(),
            args.debounce_ms,
            args.poll_ms,
            args.gas,
            args.trace,
            args.clear,
            args.once,
        ),
        CliCommand::Test(args) => commands::test::execute(&args.path, args.json, args.gas),
        CliCommand::Init(args) => commands::init::execute(
            args.dir.as_deref(),
            args.name.as_deref(),
            args.workspace,
            &args.template,
            args.force,
        ),
        CliCommand::New(args) => commands::new::execute(
            &args.path,
            args.name.as_deref(),
            args.workspace,
            &args.template,
            args.force,
        ),
    }
}
