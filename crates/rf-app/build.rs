/// "b241ace" (plus "+" when the tree had uncommitted changes) and a time, shown next to the
/// version so every copy says which code it is. The time is the commit's, so the stamp only
/// changes with a commit or a checkout; RUSTYFILES_STAMP_BUILD=1 stamps the build time instead.
///
/// Git runs with --no-optional-locks: a plain `git status` rewrites .git/index, and anything
/// watching it (as this script used to) then rebuilt rf-app on every build.
fn build_info() {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("--no-optional-locks")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let hash = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let secs = if std::env::var_os("RUSTYFILES_STAMP_BUILD").is_some() {
        now()
    } else {
        git(&["log", "-1", "--format=%ct"]).and_then(|s| s.parse().ok()).unwrap_or_else(now)
    };
    println!("cargo:rustc-env=RUSTYFILES_COMMIT={hash}{}", if dirty { "+" } else { "" });
    println!("cargo:rustc-env=RUSTYFILES_BUILT={secs}");
    println!("cargo:rerun-if-env-changed=RUSTYFILES_STAMP_BUILD");
    // a new commit or a checkout rebuilds the stamp: HEAD, the branch it points at, packed refs
    // (refs live in the common dir when this is a worktree)
    if let Some(dir) = git(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
    }
    // only files that exist: Cargo counts a missing one as changed on every build
    if let Some(common) = git(&["rev-parse", "--git-common-dir"]) {
        let branch = git(&["symbolic-ref", "-q", "HEAD"]);
        for f in std::iter::once("packed-refs".to_string()).chain(branch) {
            let path = format!("{common}/{f}");
            if std::path::Path::new(&path).exists() {
                println!("cargo:rerun-if-changed={path}");
            }
        }
    }
}

/// `locales/<code>.json` (UI translations, `{"<id>": "text"}`) embedded as
/// `LOCALES: &[(code, json)]`, sorted by code. Files starting with "_" are not locales.
fn locales() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("locales");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut found: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| {
            let code = p.file_stem()?.to_str()?.to_string();
            (!code.starts_with('_')).then_some((code, p))
        })
        .collect();
    found.sort();
    let mut out = String::from("pub const LOCALES: &[(&str, &str)] = &[\n");
    for (code, path) in &found {
        out += &format!("    ({code:?}, include_str!({:?})),\n", path.display().to_string());
    }
    out += "];\n";
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("locales.rs");
    std::fs::write(dest, out).expect("write locales.rs");
}

fn main() {
    build_info();
    locales();
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
