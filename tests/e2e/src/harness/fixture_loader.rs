use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FixtureFile {
    pub path: PathBuf,
    pub filename: String,
    pub content: String,
    pub is_valid: bool,
    pub expected_error: Option<String>,
}

pub struct FixtureLoader {
    fixtures_dir: PathBuf,
}

impl FixtureLoader {
    pub fn new() -> Self {
        // Resolve fixtures directory relative to repo root
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("e2e has parent")
            .to_path_buf();
        let fixtures_dir = root.join("fixtures");
        Self { fixtures_dir }
    }

    pub fn fixtures_dir(&self) -> &Path {
        &self.fixtures_dir
    }

    pub fn load_valid_fixtures(&self) -> Vec<FixtureFile> {
        let valid_dir = self.fixtures_dir.join("valid");
        self.load_dir(&valid_dir, true)
    }

    pub fn load_invalid_fixtures(&self) -> Vec<FixtureFile> {
        let invalid_dir = self.fixtures_dir.join("invalid");
        self.load_dir(&invalid_dir, false)
    }

    fn load_dir(&self, dir: &Path, is_valid: bool) -> Vec<FixtureFile> {
        let mut fixtures = Vec::new();
        if !dir.exists() {
            return fixtures;
        }

        let entries = fs::read_dir(dir).expect("failed to read fixtures directory");
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("ssa") {
                let filename = path.file_name().unwrap().to_string_lossy().to_string();
                let content = fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("failed to read fixture file {}: {}", path.display(), e));
                
                let expected_error = content
                    .lines()
                    .find(|line| line.starts_with("// Error:") || line.starts_with("# Error:"))
                    .map(|line| {
                        line.trim_start_matches("// Error:")
                            .trim_start_matches("# Error:")
                            .trim()
                            .to_string()
                    });

                fixtures.push(FixtureFile {
                    path,
                    filename,
                    content,
                    is_valid,
                    expected_error,
                });
            }
        }
        fixtures.sort_by(|a, b| a.filename.cmp(&b.filename));
        fixtures
    }
}

impl Default for FixtureLoader {
    fn default() -> Self {
        Self::new()
    }
}
