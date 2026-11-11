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
        let mut metadata = vec![
            "HEAD".to_string(),
            "index".to_string(),
            "packed-refs".to_string(),
        ];
        if let Some(reference) = git(root, &["symbolic-ref", "-q", "HEAD"]) {
            metadata.push(reference);
        }
        for name in metadata {
            // Linked worktrees keep HEAD/index in their actual git-dir,
            // and shared refs in the common dir; .git itself is a text file.
            if let Some(path) = git(root, &["rev-parse", "--git-path", &name]) {
                let path = Path::new(&path);
                let path = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    root.join(path)
                };
                println!("cargo:rerun-if-changed={}", path.display());
            }
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
