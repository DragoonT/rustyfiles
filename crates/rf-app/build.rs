fn main() {
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
