/// "b241ace" (plus "+" when the tree had uncommitted changes) and the UTC build time,
/// shown next to the version so every copy says which code it is.
fn build_info() {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let hash = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    println!("cargo:rustc-env=RUSTYFILES_COMMIT={hash}{}", if dirty { "+" } else { "" });
    println!("cargo:rustc-env=RUSTYFILES_BUILT={secs}");
    // a new commit or a checkout rebuilds the stamp
    if let Some(dir) = git(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/index");
    }
}

fn main() {
    build_info();
    println!("cargo:rerun-if-changed=rustyfiles.rc");
    println!("cargo:rerun-if-changed=directlook.rc");
    println!("cargo:rerun-if-changed=assets/rustyfiles.ico");
    // each exe gets its own name (Windows shows FileDescription in Open with / Task Manager)
    #[cfg(windows)]
    {
        embed_resource::compile_for("rustyfiles.rc", ["rustyfiles"], embed_resource::NONE)
            .manifest_optional()
            .expect("embed rustyfiles resources");
        embed_resource::compile_for("directlook.rc", ["directlook"], embed_resource::NONE)
            .manifest_optional()
            .expect("embed directlook resources");
    }
}
