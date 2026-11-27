//! Embed the identity of the checkout used for this build.

use std::path::PathBuf;
use std::process::Command;

fn git_identity() -> Option<(String, bool)> {
    let head = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()?;
    if !head.status.success() {
        return None;
    }
    let commit = String::from_utf8(head.stdout).ok()?.trim().to_owned();
    let status = Command::new("git")
        .args(["diff", "--quiet", "--no-ext-diff", &commit, "--", ":/"])
        .status()
        .ok()?;
    match status.code()? {
        0 => Some((commit, false)),
        1 => Some((commit, true)),
        _ => None,
    }
}

fn main() {
    // An intentionally absent input makes Cargo sample Git on every build.
    // HEAD, packed refs, the index and tracked files outside this crate can
    // all change independently, including in linked worktrees.
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    println!(
        "cargo::rerun-if-changed={}",
        out.join("always-check-git").display()
    );
    let (commit, dirty) = git_identity().unwrap_or_else(|| ("unknown".into(), true));
    println!("cargo::rustc-env=HL_BUILD_COMMIT={commit}");
    println!("cargo::rustc-env=HL_BUILD_DIRTY={dirty}");
}
