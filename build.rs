fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/alia.rc");
    println!("cargo:rerun-if-changed=assets/alia.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/alia.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("failed to embed application icon");
    }
}
