//! Cross-platform command resolution.
//!
//! Windows npm commands are commonly `.cmd` shims, while macOS applications
//! launched from Finder receive a much smaller PATH than an interactive shell.
//! Resolve both cases without starting a shell or interpolating user input.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn command(program: impl AsRef<OsStr>) -> Command {
    let program = program.as_ref();
    if let Some(path) = resolve(program) {
        return Command::new(path);
    }
    Command::new(program)
}

pub fn resolve(program: impl AsRef<OsStr>) -> Option<PathBuf> {
    let program = program.as_ref();
    let path = Path::new(program);
    if path.is_absolute() || path.components().count() > 1 {
        return resolve_candidate(path);
    }
    if let Some(paths) = std::env::var_os("PATH") {
        if let Some(resolved) = resolve_in_directories(path, std::env::split_paths(&paths)) {
            return Some(resolved);
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(resolved) = resolve_in_directories(path, macos_fallback_directories()) {
        return Some(resolved);
    }
    None
}

fn resolve_in_directories(
    program: &Path,
    directories: impl IntoIterator<Item = PathBuf>,
) -> Option<PathBuf> {
    directories
        .into_iter()
        .find_map(|directory| resolve_candidate(&directory.join(program)))
}

fn resolve_candidate(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    #[cfg(windows)]
    if path.extension().is_none() {
        for extension in windows_extensions() {
            let mut candidate = path.as_os_str().to_os_string();
            candidate.push(extension);
            let candidate = PathBuf::from(candidate);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn macos_fallback_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(home) = dirs::home_dir() {
        directories.extend([
            home.join(".local/bin"),
            home.join(".volta/bin"),
            home.join(".fnm/aliases/default/bin"),
            home.join(".npm-global/bin"),
        ]);
        let nvm_versions = home.join(".nvm/versions/node");
        if let Ok(entries) = std::fs::read_dir(nvm_versions) {
            let mut nvm_bins = entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|entry| entry.path().join("bin"))
                .collect::<Vec<_>>();
            nvm_bins.sort_by(|left, right| right.cmp(left));
            directories.extend(nvm_bins);
        }
    }
    directories.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/opt/local/bin"),
    ]);
    directories
}

#[cfg(windows)]
fn windows_extensions() -> Vec<std::ffi::OsString> {
    std::env::var_os("PATHEXT")
        .map(|value| {
            value
                .to_string_lossy()
                .split(';')
                .filter(|item| !item.is_empty())
                .map(std::ffi::OsString::from)
                .collect()
        })
        .filter(|extensions: &Vec<_>| !extensions.is_empty())
        .unwrap_or_else(|| [".COM", ".EXE", ".BAT", ".CMD"].map(Into::into).to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_an_explicit_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("meter-command");
        std::fs::write(&path, b"test").unwrap();
        assert_eq!(resolve(&path), Some(path));
    }

    #[test]
    fn directory_resolution_preserves_search_order() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let program = if cfg!(windows) {
            Path::new("meter-command.cmd")
        } else {
            Path::new("meter-command")
        };
        let first_path = first.path().join(program);
        let second_path = second.path().join(program);
        std::fs::write(&first_path, b"first").unwrap();
        std::fs::write(&second_path, b"second").unwrap();
        assert_eq!(
            resolve_in_directories(program, [first.path().into(), second.path().into()]),
            Some(first_path)
        );
    }

    #[cfg(windows)]
    #[test]
    fn resolved_cmd_shim_can_be_spawned() {
        let directory = tempfile::tempdir().unwrap();
        let shim = directory.path().join("codex.cmd");
        std::fs::write(&shim, "@echo off\r\necho codex-test 1.0\r\n").unwrap();
        let mut child = command(&shim);
        let output = child.output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "codex-test 1.0"
        );
    }
}
