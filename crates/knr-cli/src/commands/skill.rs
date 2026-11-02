use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::args::{SkillArgs, VERSION};
use crate::error::CliError;

pub const SKILL_MARKDOWN: &str = include_str!("../../../../.agents/skills/knr-ssa/SKILL.md");
pub const GEN_VDOM_SCRIPT: &str =
    include_str!("../../../../.agents/skills/knr-ssa/scripts/gen_vdom_ssa.py");

#[derive(Serialize)]
struct SkillJsonPayload<'a> {
    name: &'a str,
    version: &'a str,
    description: &'a str,
    skill_path: &'a str,
    helper_script_path: &'a str,
    skill_markdown: &'a str,
    helper_script_python: &'a str,
}

pub fn execute(args: &SkillArgs) -> Result<(), CliError> {
    if let Some(install_root) = &args.install_dir {
        let skill_dir = install_root.join(".agents").join("skills").join("knr-ssa");
        let scripts_dir = skill_dir.join("scripts");
        fs::create_dir_all(&scripts_dir)?;

        let skill_file = skill_dir.join("SKILL.md");
        let script_file = scripts_dir.join("gen_vdom_ssa.py");
        fs::write(&skill_file, SKILL_MARKDOWN)?;
        fs::write(&script_file, GEN_VDOM_SCRIPT)?;

        if args.json {
            let out = serde_json::json!({
                "status": "installed",
                "skill_name": "knr-ssa",
                "skill_md": skill_file.to_string_lossy(),
                "helper_script": script_file.to_string_lossy(),
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&out).map_err(|e| CliError::Other(e.to_string()))?
            );
        } else {
            println!(
                "Installed knr-ssa skill to {} (and helper script {})",
                skill_file.display(),
                script_file.display()
            );
        }
        return Ok(());
    }

    let rendered = if args.json {
        let payload = SkillJsonPayload {
            name: "knr-ssa",
            version: VERSION,
            description: "Official guide, ISA reference, and AI workflow for the knr linear SSA programming language and toolchain",
            skill_path: ".agents/skills/knr-ssa/SKILL.md",
            helper_script_path: ".agents/skills/knr-ssa/scripts/gen_vdom_ssa.py",
            skill_markdown: SKILL_MARKDOWN,
            helper_script_python: GEN_VDOM_SCRIPT,
        };
        serde_json::to_string_pretty(&payload).map_err(|e| CliError::Other(e.to_string()))?
    } else {
        SKILL_MARKDOWN.to_string()
    };

    if let Some(out_path) = &args.output {
        write_file_ensuring_parent(out_path, &rendered)?;
    } else {
        print!("{rendered}");
        if !rendered.ends_with('\n') {
            println!();
        }
    }

    Ok(())
}

fn write_file_ensuring_parent(path: &Path, content: &str) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(path, content)?;
    Ok(())
}
