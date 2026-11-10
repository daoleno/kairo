//! Windows resources (icon, manifest, version) for kairo.exe.

fn main() {
    println!("cargo:rerun-if-changed=windows/kairo.rc");
    println!("cargo:rerun-if-changed=windows/kairo.manifest");
    println!("cargo:rerun-if-changed=assets/kairo.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut parts: Vec<String> = version
        .split(['.', '-', '+'])
        .map(|p| {
            p.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .map(|p| if p.is_empty() { "0".into() } else { p })
        .take(3)
        .collect();
    parts.resize(3, "0".into());
    let comma = format!("{},0", parts.join(","));
    let result = embed_resource::compile(
        "windows/kairo.rc",
        [
            format!("KAIRO_VERSION_COMMA={comma}"),
            format!("KAIRO_VERSION_STR=\"{version}\""),
        ],
    );
    // With MSVC (the release build) the resources are required; a gnu
    // cross-check on a machine without windres can go without them.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        result.manifest_required().expect("windows resources");
    } else {
        let _ = result.manifest_optional();
    }
}
