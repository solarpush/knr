use std::path::Path;
use std::process::{Command, Output};

#[derive(Debug, Clone)]
pub struct CommandResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

pub struct ProcessRunner;

impl ProcessRunner {
    pub fn find_knr_binary() -> Option<String> {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let tests_dir = manifest_dir.parent().unwrap();
        let repo_root = tests_dir.parent().unwrap_or(tests_dir);

        let candidates = [
            repo_root.join("target/debug/knr"),
            repo_root.join("target/release/knr"),
            tests_dir.join("target/debug/knr"),
            tests_dir.join("target/release/knr"),
        ];

        for candidate in &candidates {
            if candidate.exists() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }

        // Check PATH
        which::which("knr").map(|p| p.to_string_lossy().to_string())
    }

    pub fn run(cmd: &str, args: &[&str]) -> Result<CommandResult, std::io::Error> {
        let output: Output = Command::new(cmd).args(args).output()?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code();
        let success = output.status.success();

        Ok(CommandResult {
            exit_code,
            stdout,
            stderr,
            success,
        })
    }
}

mod which {
    use std::path::PathBuf;
    pub fn which(name: &str) -> Option<PathBuf> {
        if let Ok(path_var) = std::env::var("PATH") {
            for dir in std::env::split_paths(&path_var) {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        None
    }
}
