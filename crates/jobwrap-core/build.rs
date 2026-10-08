use std::{env, path::Path, process::Command};

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("Cargo manifest directory");
    let root = Path::new(&manifest)
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    println!("cargo:rerun-if-changed=build.rs");
    let mut commit = "unknown".to_string();
    let mut dirty = false;
    if root.join(".git").exists() {
        // Watch Git metadata and sources so rebuilding after a commit, branch
        // switch, or tracked source edit cannot reuse stale build information.
        for path in [".git/HEAD", ".git/index", ".git/packed-refs"] {
            println!("cargo:rerun-if-changed={}", root.join(path).display());
        }
        if let Some(reference) = git(root, &["symbolic-ref", "-q", "HEAD"]) {
            println!(
                "cargo:rerun-if-changed={}",
                root.join(".git").join(reference).display()
            );
        }
        if let Some(files) = git(root, &["ls-files"]) {
            for file in files.lines() {
                println!("cargo:rerun-if-changed={}", root.join(file).display());
            }
        }
        commit = git(root, &["rev-parse", "HEAD"]).unwrap_or(commit);
        dirty = git(root, &["status", "--porcelain", "--untracked-files=normal"])
            .map_or(true, |status| !status.is_empty());
    }
    println!("cargo:rustc-env=JOBWRAP_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=JOBWRAP_BUILD_DIRTY={dirty}");
}
