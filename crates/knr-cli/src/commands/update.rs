use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::args::VERSION;
use crate::error::CliError;

const DEFAULT_REPO: &str = "solarpush/knr";

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// Returns the Rust target triple corresponding to the current OS and CPU architecture.
pub fn current_target_triple() -> Result<&'static str, CliError> {
    match (env::consts::ARCH, env::consts::OS) {
        ("x86_64", "linux") => Ok("x86_64-unknown-linux-gnu"),
        ("aarch64", "linux") => Ok("aarch64-unknown-linux-gnu"),
        ("aarch64", "macos") => Ok("aarch64-apple-darwin"),
        (arch, os) => Err(CliError::Other(format!(
            "Unsupported platform for binary self-update: {arch}-{os}"
        ))),
    }
}

/// Parses a version string such as `"v0.2.1"` or `"0.2.1-alpha"` into `(major, minor, patch)`.
pub fn parse_semver(version: &str) -> Option<(u64, u64, u64)> {
    let trimmed = version.trim().strip_prefix('v').unwrap_or(version.trim());
    let core = trimmed.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Returns `true` if `candidate` is strictly newer than `current`.
pub fn is_newer_version(current: &str, candidate: &str) -> bool {
    match (parse_semver(current), parse_semver(candidate)) {
        (Some(cur), Some(cand)) => cand > cur,
        _ => {
            let c_norm = current.trim().strip_prefix('v').unwrap_or(current.trim());
            let n_norm = candidate
                .trim()
                .strip_prefix('v')
                .unwrap_or(candidate.trim());
            c_norm != n_norm
        }
    }
}

pub fn execute(
    check_only: bool,
    force: bool,
    target_version: Option<&str>,
) -> Result<(), CliError> {
    let repo = env::var("KNR_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string());
    let api_url = if let Ok(custom_url) = env::var("KNR_RELEASE_API_URL") {
        custom_url
    } else if let Some(ver) = target_version {
        let tag = if ver.starts_with('v') {
            ver.to_string()
        } else {
            format!("v{ver}")
        };
        format!("https://api.github.com/repos/{repo}/releases/tags/{tag}")
    } else {
        format!("https://api.github.com/repos/{repo}/releases/latest")
    };

    println!("Checking releases for {repo} (current: v{VERSION})...");
    let release_json = fetch_url_string(&api_url)?;
    let release: GithubRelease = serde_json::from_str(&release_json).map_err(|e| {
        CliError::Other(format!(
            "Failed to parse GitHub release metadata from {api_url}: {e}"
        ))
    })?;

    let latest_tag = release.tag_name.trim();
    let latest_clean = latest_tag.strip_prefix('v').unwrap_or(latest_tag);

    let needs_update = if target_version.is_some() {
        force || latest_clean != VERSION
    } else {
        force || is_newer_version(VERSION, latest_tag)
    };

    if !needs_update {
        println!("knr is already up to date (v{VERSION}).");
        return Ok(());
    }

    println!("Update available: v{VERSION} -> {latest_tag}");
    if check_only {
        if !release.html_url.is_empty() {
            println!("Release notes: {}", release.html_url);
        }
        println!("Run 'knr update' to install {latest_tag}.");
        return Ok(());
    }

    let target = current_target_triple()?;
    let expected_asset_name = format!("knr-{target}.tar.gz");
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == expected_asset_name || a.name.ends_with(&format!("{target}.tar.gz")))
        .ok_or_else(|| {
            CliError::Other(format!(
                "No prebuilt binary asset found for target '{target}' in release {latest_tag}"
            ))
        })?;

    let dest_bin = if let Ok(custom_bin) = env::var("KNR_UPDATE_BIN_PATH") {
        PathBuf::from(custom_bin)
    } else {
        env::current_exe().map_err(CliError::Io)?
    };

    println!(
        "Downloading {} from {}...",
        asset.name, asset.browser_download_url
    );
    install_archive(&asset.browser_download_url, &dest_bin)?;

    println!(
        "Successfully updated knr to {latest_tag} at {}",
        dest_bin.display()
    );
    Ok(())
}

fn fetch_url_string(url: &str) -> Result<String, CliError> {
    if let Some(file_path) = url.strip_prefix("file://") {
        return fs::read_to_string(file_path).map_err(CliError::Io);
    }

    let output = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: knr-cli-updater",
            url,
        ])
        .output()
        .map_err(|e| CliError::Other(format!("Failed to execute 'curl': {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::Other(format!(
            "Failed to fetch release from {url}: {}",
            stderr.trim()
        )));
    }

    String::from_utf8(output.stdout)
        .map_err(|e| CliError::Other(format!("Invalid UTF-8 in response from {url}: {e}")))
}

fn download_to_file(url: &str, dest: &Path) -> Result<(), CliError> {
    if let Some(file_path) = url.strip_prefix("file://") {
        fs::copy(file_path, dest).map_err(CliError::Io)?;
        return Ok(());
    }

    let output = Command::new("curl")
        .args(["-fsSL", "-H", "User-Agent: knr-cli-updater", "-o"])
        .arg(dest)
        .arg(url)
        .output()
        .map_err(|e| CliError::Other(format!("Failed to execute 'curl': {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::Other(format!(
            "Failed to download asset from {url}: {}",
            stderr.trim()
        )));
    }

    Ok(())
}

fn install_archive(download_url: &str, dest_bin: &Path) -> Result<(), CliError> {
    let pid = std::process::id();
    let tmp_dir = env::temp_dir().join(format!("knr_update_{pid}"));
    if tmp_dir.exists() {
        let _ = fs::remove_dir_all(&tmp_dir);
    }
    fs::create_dir_all(&tmp_dir).map_err(CliError::Io)?;

    let archive_path = tmp_dir.join("knr.tar.gz");
    let extract_dir = tmp_dir.join("extract");
    fs::create_dir_all(&extract_dir).map_err(CliError::Io)?;

    let result = (|| -> Result<(), CliError> {
        download_to_file(download_url, &archive_path)?;

        let tar_out = Command::new("tar")
            .arg("-xzf")
            .arg(&archive_path)
            .arg("-C")
            .arg(&extract_dir)
            .output()
            .map_err(|e| CliError::Other(format!("Failed to execute 'tar': {e}")))?;

        if !tar_out.status.success() {
            let stderr = String::from_utf8_lossy(&tar_out.stderr);
            return Err(CliError::Other(format!(
                "Failed to extract archive: {}",
                stderr.trim()
            )));
        }

        let extracted_bin = find_binary_in_dir(&extract_dir, "knr").ok_or_else(|| {
            CliError::Other("Extracted release archive does not contain a 'knr' binary".to_string())
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&extracted_bin)
                .map_err(CliError::Io)?
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&extracted_bin, perms).map_err(CliError::Io)?;
        }

        // Stage next to destination binary for atomic rename on the same filesystem
        let parent_dir = dest_bin
            .parent()
            .ok_or_else(|| CliError::Other("Invalid destination binary path".to_string()))?;
        let staged_bin = parent_dir.join(format!(".knr.new.{pid}"));

        fs::copy(&extracted_bin, &staged_bin).map_err(CliError::Io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&staged_bin)
                .map_err(CliError::Io)?
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&staged_bin, perms).map_err(CliError::Io)?;
        }

        if let Err(e) = fs::rename(&staged_bin, dest_bin) {
            let _ = fs::remove_file(&staged_bin);
            return Err(CliError::Io(e));
        }

        Ok(())
    })();

    let _ = fs::remove_dir_all(&tmp_dir);
    result
}

fn find_binary_in_dir(dir: &Path, bin_name: &str) -> Option<PathBuf> {
    let direct = dir.join(bin_name);
    if direct.is_file() {
        return Some(direct);
    }
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_binary_in_dir(&path, bin_name) {
                return Some(found);
            }
        } else if path.file_name().and_then(|n| n.to_str()) == Some(bin_name) {
            return Some(path);
        }
    }
    None
}
