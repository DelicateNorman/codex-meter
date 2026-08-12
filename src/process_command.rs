//! Cross-platform command resolution.
//!
//! `std::process::Command` automatically tries an omitted `.exe` suffix on
//! Windows, but npm-installed commands are commonly exposed as `.cmd` shims.
//! Resolve every PATHEXT entry first so `codex` works the same way it does in
//! PowerShell and cmd.exe.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn command(program: impl AsRef<OsStr>) -> Command {
    let program = program.as_ref();
    #[cfg(windows)]
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
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|directory| resolve_candidate(&directory.join(path)))
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
