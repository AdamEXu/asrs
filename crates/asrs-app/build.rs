#![allow(clippy::disallowed_methods)]

fn main() {
    println!("cargo:rerun-if-env-changed=ASRS_PACKAGED");
    println!("cargo:rerun-if-env-changed=ASRS_BUILD_DATE");
    println!("cargo:rerun-if-env-changed=ASRS_FILE_VERSION");
    println!("cargo:rerun-if-changed=../../packaging/icons/asrs.ico");
    #[cfg(windows)]
    if std::env::var_os("ASRS_PACKAGED").is_some() {
        windows();
    }
}

#[cfg(windows)]
fn windows() {
    println!("cargo:rustc-link-arg-bins=/SUBSYSTEM:WINDOWS");
    println!("cargo:rustc-link-arg-bins=/ENTRY:mainCRTStartup");
    let version = std::env::var("ASRS_FILE_VERSION").expect("a packaged build's ASRS_FILE_VERSION");
    let fields: Vec<u16> = version
        .split('.')
        .map(|field| field.parse().expect("a field of ASRS_FILE_VERSION"))
        .collect();
    assert_eq!(fields.len(), 4, "ASRS_FILE_VERSION is {version}");
    let packed = fields
        .iter()
        .fold(0u64, |packed, field| (packed << 16) | u64::from(*field));
    winresource::WindowsResource::new()
        .set_version_info(winresource::VersionInfo::FILEVERSION, packed)
        .set_version_info(winresource::VersionInfo::PRODUCTVERSION, packed)
        .set_icon("../../packaging/icons/asrs.ico")
        .set("ProductName", "asrs")
        .set("FileDescription", "asrs")
        .set(
            "LegalCopyright",
            "Copyright (c) 2026 Adam Xu. Copyright (c) 2021-2026 Littleton Robotics.",
        )
        .compile()
        .expect("compiling the Windows resources");
}
