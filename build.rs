use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn git(root: &Path, arguments: &[&str]) -> Option<Output> {
    let program = if cfg!(target_os = "macos") {
        "/usr/bin/git"
    } else {
        "git"
    };
    Command::new(program)
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .ok()
}

fn git_stdout(root: &Path, arguments: &[&str]) -> Option<String> {
    git(root, arguments)
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn absolute_git_path(root: &Path, value: String) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let commit = git_stdout(&root, &["rev-parse", "--verify", "HEAD"])
        .filter(|value| {
            matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .unwrap_or_else(|| "unknown".to_string());
    let status_clean = git(
        &root,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )
    .is_some_and(|output| output.status.success() && output.stdout.is_empty());
    let tracked_paths = git(&root, &["ls-files", "-z"])
        .filter(|output| output.status.success())
        .and_then(|output| {
            output
                .stdout
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty())
                .map(|path| {
                    let path = std::str::from_utf8(path).ok()?;
                    (!path.contains(['\n', '\r'])).then_some(path.to_string())
                })
                .collect::<Option<Vec<_>>>()
        });
    // If Cargo cannot watch every tracked path, a later source edit could
    // leave clean build metadata stale. Such a build is never release-clean.
    let clean = status_clean && tracked_paths.is_some();

    println!("cargo:rustc-env=COMMANDER_BUILD_GIT_COMMIT={commit}");
    println!(
        "cargo:rustc-env=COMMANDER_BUILD_GIT_DIRTY={}",
        if clean { "false" } else { "true" }
    );

    // Rebuild metadata after a commit, checkout, staging change, or branch move.
    for arguments in [
        &["rev-parse", "--git-path", "HEAD"][..],
        &["rev-parse", "--git-path", "index"][..],
    ] {
        if let Some(path) = git_stdout(&root, arguments) {
            println!(
                "cargo:rerun-if-changed={}",
                absolute_git_path(&root, path).display()
            );
        }
    }
    if let Some(reference) = git_stdout(&root, &["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git_stdout(&root, &["rev-parse", "--git-path", &reference])
    {
        println!(
            "cargo:rerun-if-changed={}",
            absolute_git_path(&root, path).display()
        );
    }

    // Emitting any rerun-if-changed disables Cargo's default package-wide
    // watch. Restore it explicitly so an unstaged tracked edit rebuilds the
    // embedded dirty bit, and reverting that edit rebuilds it back to clean.
    if let Some(paths) = tracked_paths {
        for path in paths {
            println!("cargo:rerun-if-changed={}", root.join(path).display());
        }
    }
}
