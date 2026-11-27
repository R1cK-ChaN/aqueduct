//! Manifest paths and publication of a completed session.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use hl_backtest::recording::{Recording, sha256_file_checked};
use hl_run::session::{ManifestEntry, Session};

pub(crate) fn manifest(recording: &Recording, output: &Path) -> io::Result<Vec<ManifestEntry>> {
    let output = std::path::absolute(output)?;
    let directory = output
        .parent()
        .ok_or_else(|| io::Error::other("expected session file name"))?;
    let prefix = path_to_working_directory(&directory.canonicalize()?)?;
    recording
        .files
        .iter()
        .enumerate()
        .map(|(index, (path, file_seq))| {
            // Keep absolute paths absolute. All relative inputs gain the same
            // prefix, preserving the original path ordering on tied headers.
            let mut name = PathBuf::new();
            if path.components().next() == Some(Component::CurDir) {
                name.push(".");
            }
            name.push(&prefix);
            name.push(path);
            Ok(ManifestEntry {
                path: name
                    .to_str()
                    .ok_or_else(|| io::Error::other("session manifest paths must be UTF-8"))?
                    .to_owned(),
                file_seq: *file_seq,
                sha256: sha256_file_checked(
                    path,
                    recording.file_stamps.get(index).ok_or_else(|| {
                        io::Error::other("recording is missing a verified raw file identity")
                    })?,
                )
                .map_err(|e| io::Error::other(format!("{}: {e}", path.display())))?,
            })
        })
        .collect()
}

fn path_to_working_directory(directory: &Path) -> io::Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let shared = directory
        .ancestors()
        .find(|ancestor| cwd.starts_with(ancestor))
        .ok_or_else(|| {
            io::Error::other("working and session directories have no common ancestor")
        })?;
    let mut prefix: PathBuf = directory
        .strip_prefix(shared)
        .expect("ancestor")
        .components()
        .map(|_| "..")
        .collect();
    prefix.push(cwd.strip_prefix(shared).expect("ancestor"));
    Ok(prefix)
}

pub(crate) fn require_unused(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", path.display()),
        )),
    }
}

pub(crate) fn seal(path: &Path, session: &Session) -> io::Result<()> {
    let mut name = OsString::from(path);
    name.push(".part");
    let part = PathBuf::from(name);
    let mut output = BufWriter::new(File::create_new(&part)?);
    for line in session.lines() {
        output.write_all(line.as_bytes())?;
    }
    output.flush()?;
    drop(output);
    // GNU mv uses a no-replace rename on Linux. A concurrent destination
    // wins even if it appeared after the initial existence check.
    let moved = Command::new("mv")
        .args(["-vnT", "--"])
        .arg(&part)
        .arg(path)
        .output()?;
    if !moved.status.success() {
        return Err(io::Error::other(format!(
            "failed to seal {}: {}",
            path.display(),
            String::from_utf8_lossy(&moved.stderr)
        )));
    }
    // Older coreutils returns success when -n skips an existing destination.
    if moved.stdout.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", path.display()),
        ));
    }
    Ok(())
}
