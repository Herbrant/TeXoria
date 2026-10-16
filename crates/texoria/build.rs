fn main() {
    println!("cargo:rerun-if-changed=packaging/texoria.rc");
    println!("cargo:rerun-if-changed=packaging/texoria.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("packaging/texoria.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
