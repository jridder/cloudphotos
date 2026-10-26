//! On Windows, embed the icon and version details in the executable.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=windows/cloudphotos.rc");
    println!("cargo:rerun-if-changed=windows/cloudphotos.ico");
    let version = |key: &str| format!("VERSION_{key}={}", std::env::var(format!("CARGO_PKG_VERSION_{key}")).unwrap());
    let macros = [
        version("MAJOR"),
        version("MINOR"),
        version("PATCH"),
        format!("VERSION_STRING=\\\"{}\\\"", std::env::var("CARGO_PKG_VERSION").unwrap()),
    ];
    embed_resource::compile("windows/cloudphotos.rc", &macros).manifest_optional().unwrap();
}
