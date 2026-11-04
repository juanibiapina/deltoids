//! Which files Jev never sees.

/// Lockfiles and generated files: large, and they say nothing about the
/// change.
pub(super) fn never_sent(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    is_lockfile(name) || is_generated(name)
}

/// Package manager lockfiles, tagged `lockfile` without asking Jev.
pub(super) fn is_lockfile_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    is_lockfile(lower.rsplit('/').next().unwrap_or(&lower))
}

fn is_lockfile(name: &str) -> bool {
    name.ends_with(".lock")
        || name.ends_with(".lockfile")
        || name.ends_with(".lockb")
        || name.ends_with(".lock.json")
        || name.ends_with(".lock.hcl")
        || matches!(
            name,
            "package-lock.json"
                | "pnpm-lock.yaml"
                | "go.sum"
                | "npm-shrinkwrap.json"
                | "package.resolved"
        )
}

fn is_generated(name: &str) -> bool {
    name.contains(".min.") || name.ends_with(".map") || name.ends_with(".snap")
}
