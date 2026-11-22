use std::fs;

use serde_json::json;

use crate::args::FixArgs;
use crate::commands::lsp::{apply_lsp_text_edits, compute_all_quickfixes_for_source};
use crate::error::CliError;

/// Executes `knr fix`: runs static analysis and deterministically proposes or applies
/// compiler-verified QuickFixes (`readonly` borrows, `@effects(...)`, missing `std/ui` imports,
/// `@shader` effect removal, `BorrowEscape` ownership fix).
pub fn execute(args: &FixArgs) -> Result<(), CliError> {
    let file = &args.file;
    if !file.is_file() {
        return Err(CliError::Other(format!(
            "Source file '{}' not found",
            file.display()
        )));
    }
    let abs = file.canonicalize().unwrap_or_else(|_| file.clone());
    let uri = format!("file://{}", abs.display());

    let original = fs::read_to_string(file).map_err(|e| {
        eprintln!("Error reading '{}': {e}", file.display());
        CliError::Io(e)
    })?;

    let mut current = original.clone();
    let mut applied_fixes: Vec<serde_json::Value> = Vec::new();

    // Iteratively apply up to 8 deterministic QuickFix passes until no auto-fixable diagnostics remain
    for _pass in 0..8 {
        let actions = compute_all_quickfixes_for_source(&uri, &current);
        let Some(first) = actions.into_iter().next() else {
            break;
        };
        let title = first["title"].as_str().unwrap_or("QuickFix").to_string();
        let edits = first
            .pointer("/edit/changes")
            .and_then(|m| m.as_object())
            .and_then(|m| m.get(&uri).or_else(|| m.values().next()))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if edits.is_empty() {
            break;
        }
        let next = apply_lsp_text_edits(&current, &edits);
        if next == current {
            break;
        }
        applied_fixes.push(json!({
            "title": title,
            "edit_count": edits.len(),
        }));
        current = next;
    }

    let changed = current != original;
    if args.write && changed {
        fs::write(file, &current).map_err(|e| {
            eprintln!("Error writing '{}': {e}", file.display());
            CliError::Io(e)
        })?;
    }

    // Re-check remaining diagnostics on the patched source
    let remaining_actions = compute_all_quickfixes_for_source(&uri, &current);

    if args.json {
        let payload = json!({
            "file": file.display().to_string(),
            "written": args.write && changed,
            "changed": changed,
            "applied_count": applied_fixes.len(),
            "applied_fixes": applied_fixes,
            "remaining_fixable": remaining_actions.len(),
            "patched_source": if !args.write && changed { Some(current) } else { None },
        });
        let pretty = serde_json::to_string_pretty(&payload)
            .map_err(|e| CliError::Other(e.to_string()))?;
        println!("{pretty}");
    } else if applied_fixes.is_empty() {
        println!("No auto-fixable diagnostics found in '{}'.", file.display());
    } else {
        for fix in &applied_fixes {
            let title = fix["title"].as_str().unwrap_or("QuickFix");
            if args.write {
                println!("  [fixed] {title}");
            } else {
                println!("  [available] {title}");
            }
        }
        if args.write {
            println!(
                "Applied {} fix(es) to '{}'.",
                applied_fixes.len(),
                file.display()
            );
        } else {
            println!(
                "Found {} fix(es) for '{}'. Re-run with `--write` to apply.",
                applied_fixes.len(),
                file.display()
            );
        }
    }

    Ok(())
}
