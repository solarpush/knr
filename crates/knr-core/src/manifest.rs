//! Manifest data model and parser for `knr.toml` projects and workspaces.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// Top-level manifest definition for a knr project or workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum Manifest {
    /// A single package manifest containing package metadata, dependencies, and entrypoint.
    Package(PackageManifest),
    /// A multi-project workspace manifest coordinating multiple member packages.
    Workspace(WorkspaceManifest),
}

/// Manifest configuration for a single package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageManifest {
    /// Package name.
    pub name: String,
    /// Semantic version string (e.g. "0.1.0").
    pub version: String,
    /// Path to the entry source file (defaults to `src/main.knr`).
    pub entry: PathBuf,
    /// Language edition (defaults to "2026").
    pub edition: String,
    /// Map of package dependencies.
    pub dependencies: BTreeMap<String, DependencySpec>,
    /// Optional short description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional list of authors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    /// Optional build settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildConfig>,
    /// Optional UI launcher configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<UiConfig>,
}

/// Manifest configuration for a multi-project workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceManifest {
    /// List of member path patterns (supporting direct relative paths and glob wildcards like `"apps/*"`).
    pub members: Vec<String>,
    /// Optional default members to target when running commands at workspace root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_members: Option<Vec<String>>,
    /// Shared dependencies available to workspace members.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, DependencySpec>,
}

/// Dependency specification supporting simple version/path strings or detailed table specifications.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DependencySpec {
    /// Simple specifier, e.g. `"0.1.0"` or `"../path/to/pkg"`.
    Simple(String),
    /// Detailed table specifier with path, version, git, or tag.
    Detailed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        git: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
    },
}

impl DependencySpec {
    /// Returns the local path if this dependency is path-based.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Simple(s) => {
                let p = Path::new(s);
                if p.is_relative() && (s.starts_with('.') || s.contains('/') || s.contains('\\')) {
                    Some(p)
                } else {
                    None
                }
            }
            Self::Detailed { path, .. } => path.as_deref(),
        }
    }

    /// Returns the version string if specified.
    pub fn version(&self) -> Option<&str> {
        match self {
            Self::Simple(s) => {
                if !s.starts_with('.') && !s.contains('/') && !s.contains('\\') {
                    Some(s.as_str())
                } else {
                    None
                }
            }
            Self::Detailed { version, .. } => version.as_deref(),
        }
    }
}

/// Optional build configuration in `knr.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BuildConfig {
    /// Build target mode ("native", "wasm", "vm").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Optimization level (0, 1, 2, 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opt_level: Option<u8>,
    /// Target output directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_dir: Option<PathBuf>,
}

/// Optional UI configuration in `knr.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UiConfig {
    /// Window title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Window width in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Window height in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Visual theme (e.g. "dark", "light").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

/// Errors occurring during manifest discovery, loading, parsing, or serialization.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("Failed to parse manifest TOML: {0}")]
    Parse(String),

    #[error("Manifest missing required root table: either [package] or [workspace] must be present")]
    MissingRootTable,

    #[error("Manifest missing required field '{0}'")]
    MissingField(&'static str),

    #[error("Manifest IO error at '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to resolve workspace member: {0}")]
    MemberResolution(String),
}

impl PackageManifest {
    /// Creates a new `PackageManifest` with default entry (`src/main.knr`) and edition (`2026`).
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            entry: PathBuf::from("src/main.knr"),
            edition: "2026".to_string(),
            dependencies: BTreeMap::new(),
            description: None,
            authors: Vec::new(),
            build: None,
            ui: None,
        }
    }
}

impl WorkspaceManifest {
    /// Creates a new `WorkspaceManifest` with the given member patterns.
    pub fn new(members: Vec<String>) -> Self {
        Self {
            members,
            default_members: None,
            dependencies: BTreeMap::new(),
        }
    }
}

impl Manifest {
    /// Parses a manifest from a TOML string.
    pub fn from_toml(src: &str) -> Result<Self, ManifestError> {
        let doc: toml_edit::DocumentMut = src
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| ManifestError::Parse(e.to_string()))?;

        let has_package = doc.get("package").and_then(|item| item.as_table()).is_some();
        let has_workspace = doc.get("workspace").and_then(|item| item.as_table()).is_some();

        if has_package {
            let pkg_table = doc["package"].as_table().unwrap();

            let name = pkg_table
                .get("name")
                .and_then(|item| item.as_str())
                .ok_or(ManifestError::MissingField("package.name"))?
                .to_string();

            let version = pkg_table
                .get("version")
                .and_then(|item| item.as_str())
                .ok_or(ManifestError::MissingField("package.version"))?
                .to_string();

            let entry = pkg_table
                .get("entry")
                .and_then(|item| item.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("src/main.knr"));

            let edition = pkg_table
                .get("edition")
                .and_then(|item| item.as_str())
                .unwrap_or("2026")
                .to_string();

            let description = pkg_table
                .get("description")
                .and_then(|item| item.as_str())
                .map(ToString::to_string);

            let authors = pkg_table
                .get("authors")
                .and_then(|item| item.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|val| val.as_str().map(ToString::to_string))
                        .collect()
                })
                .unwrap_or_default();

            // Parse dependencies from root [dependencies] or [package.dependencies]
            let mut dependencies = BTreeMap::new();
            if let Some(deps_table) = doc
                .get("dependencies")
                .and_then(|item| item.as_table())
                .or_else(|| pkg_table.get("dependencies").and_then(|item| item.as_table()))
            {
                for (dep_name, dep_item) in deps_table.iter() {
                    if let Some(spec) = parse_dependency_spec(dep_item) {
                        dependencies.insert(dep_name.to_string(), spec);
                    }
                }
            }

            // Parse [build] if present
            let build = doc.get("build").and_then(|item| item.as_table()).map(|tbl| {
                let target = tbl.get("target").and_then(|v| v.as_str()).map(ToString::to_string);
                let opt_level = tbl
                    .get("opt_level")
                    .and_then(|v| v.as_integer())
                    .and_then(|i| u8::try_from(i).ok());
                let out_dir = tbl.get("out_dir").and_then(|v| v.as_str()).map(PathBuf::from);
                BuildConfig { target, opt_level, out_dir }
            });

            // Parse [ui] if present
            let ui = doc.get("ui").and_then(|item| item.as_table()).map(|tbl| {
                let title = tbl.get("title").and_then(|v| v.as_str()).map(ToString::to_string);
                let width = tbl
                    .get("width")
                    .and_then(|v| v.as_integer())
                    .and_then(|i| u32::try_from(i).ok());
                let height = tbl
                    .get("height")
                    .and_then(|v| v.as_integer())
                    .and_then(|i| u32::try_from(i).ok());
                let theme = tbl.get("theme").and_then(|v| v.as_str()).map(ToString::to_string);
                UiConfig { title, width, height, theme }
            });

            Ok(Manifest::Package(PackageManifest {
                name,
                version,
                entry,
                edition,
                dependencies,
                description,
                authors,
                build,
                ui,
            }))
        } else if has_workspace {
            let ws_table = doc["workspace"].as_table().unwrap();

            let members = ws_table
                .get("members")
                .and_then(|item| item.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|val| val.as_str().map(ToString::to_string))
                        .collect()
                })
                .unwrap_or_default();

            let default_members = if let Some(arr) = ws_table.get("default_members").and_then(|item| item.as_array()) {
                Some(arr.iter().filter_map(|val| val.as_str().map(ToString::to_string)).collect())
            } else {
                ws_table
                    .get("default_member")
                    .and_then(|item| item.as_str())
                    .map(|single| vec![single.to_string()])
            };

            let mut dependencies = BTreeMap::new();
            if let Some(deps_table) = ws_table
                .get("dependencies")
                .and_then(|item| item.as_table())
                .or_else(|| doc.get("dependencies").and_then(|item| item.as_table()))
            {
                for (dep_name, dep_item) in deps_table.iter() {
                    if let Some(spec) = parse_dependency_spec(dep_item) {
                        dependencies.insert(dep_name.to_string(), spec);
                    }
                }
            }

            Ok(Manifest::Workspace(WorkspaceManifest {
                members,
                default_members,
                dependencies,
            }))
        } else {
            Err(ManifestError::MissingRootTable)
        }
    }

    /// Serializes the manifest into a formatted TOML string.
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        match self {
            Manifest::Package(pkg) => {
                out.push_str("[package]\n");
                out.push_str(&format!("name = \"{}\"\n", escape_toml_str(&pkg.name)));
                out.push_str(&format!("version = \"{}\"\n", escape_toml_str(&pkg.version)));
                out.push_str(&format!("entry = \"{}\"\n", escape_toml_str(&pkg.entry.to_string_lossy())));
                out.push_str(&format!("edition = \"{}\"\n", escape_toml_str(&pkg.edition)));

                if let Some(desc) = &pkg.description {
                    out.push_str(&format!("description = \"{}\"\n", escape_toml_str(desc)));
                }

                if !pkg.authors.is_empty() {
                    out.push_str("authors = [");
                    for (i, author) in pkg.authors.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        out.push_str(&format!("\"{}\"", escape_toml_str(author)));
                    }
                    out.push_str("]\n");
                }

                if !pkg.dependencies.is_empty() {
                    out.push_str("\n[dependencies]\n");
                    for (dep_name, spec) in &pkg.dependencies {
                        out.push_str(&format_dependency_entry(dep_name, spec));
                    }
                }

                if let Some(build) = &pkg.build {
                    out.push_str("\n[build]\n");
                    if let Some(t) = &build.target {
                        out.push_str(&format!("target = \"{}\"\n", escape_toml_str(t)));
                    }
                    if let Some(opt) = build.opt_level {
                        out.push_str(&format!("opt_level = {}\n", opt));
                    }
                    if let Some(d) = &build.out_dir {
                        out.push_str(&format!("out_dir = \"{}\"\n", escape_toml_str(&d.to_string_lossy())));
                    }
                }

                if let Some(ui) = &pkg.ui {
                    out.push_str("\n[ui]\n");
                    if let Some(title) = &ui.title {
                        out.push_str(&format!("title = \"{}\"\n", escape_toml_str(title)));
                    }
                    if let Some(w) = ui.width {
                        out.push_str(&format!("width = {}\n", w));
                    }
                    if let Some(h) = ui.height {
                        out.push_str(&format!("height = {}\n", h));
                    }
                    if let Some(theme) = &ui.theme {
                        out.push_str(&format!("theme = \"{}\"\n", escape_toml_str(theme)));
                    }
                }
            }
            Manifest::Workspace(ws) => {
                out.push_str("[workspace]\n");
                out.push_str("members = [\n");
                for member in &ws.members {
                    out.push_str(&format!("    \"{}\",\n", escape_toml_str(member)));
                }
                out.push_str("]\n");

                if let Some(defaults) = &ws.default_members {
                    if defaults.is_empty() {
                        out.push_str("default_members = []\n");
                    } else {
                        out.push_str("default_members = [\n");
                        for member in defaults {
                            out.push_str(&format!("    \"{}\",\n", escape_toml_str(member)));
                        }
                        out.push_str("]\n");
                    }
                }

                if !ws.dependencies.is_empty() {
                    out.push_str("\n[workspace.dependencies]\n");
                    for (dep_name, spec) in &ws.dependencies {
                        out.push_str(&format_dependency_entry(dep_name, spec));
                    }
                }
            }
        }
        out
    }

    /// Loads and parses a manifest from a file path or directory containing `knr.toml`.
    pub fn load_from_path(path: &Path) -> Result<Self, ManifestError> {
        let manifest_file = if path.is_dir() {
            path.join("knr.toml")
        } else {
            path.to_path_buf()
        };

        let content = fs::read_to_string(&manifest_file).map_err(|source| ManifestError::Io {
            path: manifest_file.clone(),
            source,
        })?;

        Self::from_toml(&content)
    }

    /// Returns `true` if this manifest is a single package.
    pub fn is_package(&self) -> bool {
        matches!(self, Manifest::Package(_))
    }

    /// Returns `true` if this manifest is a workspace.
    pub fn is_workspace(&self) -> bool {
        matches!(self, Manifest::Workspace(_))
    }

    /// Borrows the package manifest if this is a package.
    pub fn as_package(&self) -> Option<&PackageManifest> {
        match self {
            Manifest::Package(p) => Some(p),
            _ => None,
        }
    }

    /// Mutably borrows the package manifest if this is a package.
    pub fn as_package_mut(&mut self) -> Option<&mut PackageManifest> {
        match self {
            Manifest::Package(p) => Some(p),
            _ => None,
        }
    }

    /// Borrows the workspace manifest if this is a workspace.
    pub fn as_workspace(&self) -> Option<&WorkspaceManifest> {
        match self {
            Manifest::Workspace(w) => Some(w),
            _ => None,
        }
    }

    /// Mutably borrows the workspace manifest if this is a workspace.
    pub fn as_workspace_mut(&mut self) -> Option<&mut WorkspaceManifest> {
        match self {
            Manifest::Workspace(w) => Some(w),
            _ => None,
        }
    }

    /// Returns package name if this is a package manifest.
    pub fn name(&self) -> Option<&str> {
        self.as_package().map(|p| p.name.as_str())
    }

    /// Returns package entry file if this is a package manifest.
    pub fn entry(&self) -> Option<&Path> {
        self.as_package().map(|p| p.entry.as_path())
    }
}

/// Helper function to parse a dependency item from `toml_edit`.
fn parse_dependency_spec(item: &toml_edit::Item) -> Option<DependencySpec> {
    if let Some(s) = item.as_str() {
        return Some(DependencySpec::Simple(s.to_string()));
    }

    if let Some(tbl) = item.as_table() {
        let path = tbl.get("path").and_then(|v| v.as_str()).map(PathBuf::from);
        let version = tbl.get("version").and_then(|v| v.as_str()).map(ToString::to_string);
        let git = tbl.get("git").and_then(|v| v.as_str()).map(ToString::to_string);
        let tag = tbl.get("tag").and_then(|v| v.as_str()).map(ToString::to_string);
        return Some(DependencySpec::Detailed { path, version, git, tag });
    }

    if let Some(tbl) = item.as_inline_table() {
        let path = tbl.get("path").and_then(|v| v.as_str()).map(PathBuf::from);
        let version = tbl.get("version").and_then(|v| v.as_str()).map(ToString::to_string);
        let git = tbl.get("git").and_then(|v| v.as_str()).map(ToString::to_string);
        let tag = tbl.get("tag").and_then(|v| v.as_str()).map(ToString::to_string);
        return Some(DependencySpec::Detailed { path, version, git, tag });
    }

    None
}

/// Formats a dependency specification as a TOML inline value.
fn format_dependency_spec(spec: &DependencySpec) -> String {
    match spec {
        DependencySpec::Simple(s) => format!("\"{}\"", escape_toml_str(s)),
        DependencySpec::Detailed { path, version, git, tag } => {
            let mut parts = Vec::new();
            if let Some(p) = path {
                parts.push(format!("path = \"{}\"", escape_toml_str(&p.to_string_lossy())));
            }
            if let Some(v) = version {
                parts.push(format!("version = \"{}\"", escape_toml_str(v)));
            }
            if let Some(g) = git {
                parts.push(format!("git = \"{}\"", escape_toml_str(g)));
            }
            if let Some(t) = tag {
                parts.push(format!("tag = \"{}\"", escape_toml_str(t)));
            }
            format!("{{ {} }}", parts.join(", "))
        }
    }
}

/// Returns true if a key is a valid unquoted bare TOML key.
fn is_bare_toml_key(k: &str) -> bool {
    !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Formats a dependency key-value pair, quoting the key if it contains dots or non-bare characters.
fn format_dependency_entry(name: &str, spec: &DependencySpec) -> String {
    if is_bare_toml_key(name) {
        format!("{} = {}\n", name, format_dependency_spec(spec))
    } else {
        format!("\"{}\" = {}\n", escape_toml_str(name), format_dependency_spec(spec))
    }
}

/// Escapes special characters for a TOML basic string.
fn escape_toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\x08' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\x0C' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) <= 0x1F || c == '\x7F' => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            other => out.push(other),
        }
    }
    out
}

/// Searches upwards starting from `start` for a directory containing `knr.toml`.
///
/// If `start` is a file, the search begins at its parent directory.
pub fn find_manifest_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };

    loop {
        if current.join("knr.toml").is_file() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Resolves workspace member path patterns into concrete directory paths containing `knr.toml`.
///
/// Supports exact relative paths (e.g. `"crates/knr-core"`) and wildcard patterns (e.g. `"apps/*"`).
pub fn resolve_workspace_members(root: &Path, members: &[String]) -> Result<Vec<PathBuf>, ManifestError> {
    let mut results = Vec::new();

    for pattern in members {
        let pattern_trimmed = pattern.trim().trim_end_matches('/');
        if pattern_trimmed.is_empty() || pattern_trimmed == "." {
            continue;
        }

        if pattern_trimmed.contains('*') {
            // Glob wildcard pattern
            expand_glob(root, pattern_trimmed, &mut results)?;
        } else {
            // Explicit relative path
            let member_path = root.join(pattern_trimmed);
            if let (Ok(c_member), Ok(c_root)) = (member_path.canonicalize(), root.canonicalize()) {
                if c_member == c_root {
                    continue;
                }
            }
            if !member_path.exists() {
                return Err(ManifestError::MemberResolution(format!(
                    "Explicit workspace member path '{}' does not exist at '{}'",
                    pattern,
                    member_path.display()
                )));
            }
            if !member_path.join("knr.toml").is_file() {
                return Err(ManifestError::MemberResolution(format!(
                    "Explicit workspace member '{}' at '{}' does not contain a knr.toml file",
                    pattern,
                    member_path.display()
                )));
            }
            results.push(member_path);
        }
    }

    // Deduplicate and sort
    results.sort();
    results.dedup();

    Ok(results)
}

/// Expands glob patterns such as `apps/*` or `*` or `packages/**` relative to `root`.
fn expand_glob(root: &Path, pattern: &str, results: &mut Vec<PathBuf>) -> Result<(), ManifestError> {
    if pattern == "*" {
        // Direct child directories of root
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !file_name.starts_with('.') && file_name != "target" && path.join("knr.toml").is_file() {
                        results.push(path);
                    }
                }
            }
        }
        return Ok(());
    }

    // Handle recursive pattern "dir/**"
    if let Some((parent_part, "")) = pattern.split_once("/**") {
        let parent_dir = root.join(parent_part);
        if parent_dir.is_dir() {
            let mut visited = HashSet::new();
            if let Ok(canonical) = parent_dir.canonicalize() {
                visited.insert(canonical);
            }
            scan_recursive_for_manifests(&parent_dir, results, &mut visited)?;
        }
        return Ok(());
    }

    // Handle patterns like "dir/*"
    if let Some((parent_part, "")) = pattern.split_once("/*") {
        let parent_dir = root.join(parent_part);
        if parent_dir.is_dir() {
            if let Ok(entries) = fs::read_dir(&parent_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if !name.starts_with('.') && name != "target" && path.join("knr.toml").is_file() {
                            results.push(path);
                        }
                    }
                }
            }
        }
        return Ok(());
    }

    // Fallback: search directory recursively if pattern ends with **
    if pattern == "**" {
        let mut visited = HashSet::new();
        if let Ok(canonical) = root.canonicalize() {
            visited.insert(canonical);
        }
        scan_recursive_for_manifests(root, results, &mut visited)?;
        return Ok(());
    }

    // General pattern: split by '/' and match segments
    let segments: Vec<&str> = pattern.split('/').collect();
    walk_glob_segments(root, &segments, results)?;

    Ok(())
}

fn walk_glob_segments(current: &Path, segments: &[&str], results: &mut Vec<PathBuf>) -> Result<(), ManifestError> {
    if segments.is_empty() {
        if current.join("knr.toml").is_file() {
            results.push(current.to_path_buf());
        }
        return Ok(());
    }

    let head = segments[0];
    let tail = &segments[1..];

    if head == "*" {
        if let Ok(entries) = fs::read_dir(current) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') && name != "target" {
                        walk_glob_segments(&path, tail, results)?;
                    }
                }
            }
        }
    } else if head == "**" {
        // Match 0 or more directory levels
        walk_glob_segments(current, tail, results)?;
        if let Ok(entries) = fs::read_dir(current) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') && name != "target" {
                        walk_glob_segments(&path, segments, results)?;
                    }
                }
            }
        }
    } else {
        let next = current.join(head);
        if next.is_dir() {
            walk_glob_segments(&next, tail, results)?;
        }
    }

    Ok(())
}

fn scan_recursive_for_manifests(
    dir: &Path,
    results: &mut Vec<PathBuf>,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), ManifestError> {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with('.') || name == "target" {
                    continue;
                }
                if let Ok(canonical) = path.canonicalize() {
                    if !visited.insert(canonical) {
                        continue;
                    }
                }
                if path.join("knr.toml").is_file() {
                    results.push(path.clone());
                }
                scan_recursive_for_manifests(&path, results, visited)?;
            }
        }
    }
    Ok(())
}
