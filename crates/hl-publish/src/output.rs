//! Install only complete manifests. An identical local retry can share the result.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

fn destination(path: &Path) -> io::Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let name = path.file_name().ok_or(err)?;
            let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
            Ok(destination(parent)?.join(name))
        }
        Err(err) => Err(err),
    }
}

fn scratch(dir: &Path) -> io::Result<(PathBuf, File)> {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    loop {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(".manifest-{}-{id}.part", std::process::id()));
        match File::create_new(&path) {
            Ok(file) => return Ok((path, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
}

pub(crate) fn write_manifest(dir: &Path, source: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = destination(&std::path::absolute(dir)?)?;
    if dir.starts_with(source.canonicalize()?) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    fs::create_dir_all(&dir)?;
    let target = dir.join("manifest.json");
    let (part, mut file) = scratch(&dir)?;
    let install = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        match fs::hard_link(&part, &target) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                let old = fs::symlink_metadata(&target)?;
                if old.is_file() && old.len() == bytes.len() as u64 && fs::read(target)? == bytes {
                    Ok(())
                } else {
                    Err(err)
                }
            }
            Err(err) => Err(err),
        }
    })();
    fs::remove_file(part)?;
    install?;
    File::open(dir)?.sync_all()
}
