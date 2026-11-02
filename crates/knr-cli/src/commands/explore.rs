use knr_core::ExploreFilter;

use crate::args::{ExploreArgs, ExploreFormat};
use crate::error::CliError;

pub fn execute(args: &ExploreArgs) -> Result<(), CliError> {
    let graph = knr_core::explore_path(&args.path).map_err(|err| {
        eprintln!("{err}");
        CliError::Parse(err)
    })?;

    let filter = ExploreFilter {
        symbol: args.symbol.clone(),
        callers: args.callers.clone(),
        callees: args.callees.clone(),
        effects: args.effects.clone(),
        orphans_only: args.orphans,
    };

    let rendered = match args.format {
        ExploreFormat::Ai => graph.format_ai(&filter),
        ExploreFormat::Json => graph.format_json(&filter),
        ExploreFormat::Mermaid => graph.format_mermaid(&filter),
    };

    if rendered.ends_with('\n') {
        print!("{rendered}");
    } else {
        println!("{rendered}");
    }

    Ok(())
}
