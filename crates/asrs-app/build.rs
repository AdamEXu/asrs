fn main() {
    println!("cargo:rerun-if-env-changed=ASRS_PACKAGED");
    println!("cargo:rerun-if-env-changed=ASRS_BUILD_DATE");
    println!("cargo:rerun-if-changed=../../packaging/icons/asrs.ico");
    if std::env::var_os("ASRS_PACKAGED").is_none() {
        return;
    }
    #[cfg(windows)]
    windows();
}

#[cfg(windows)]
fn windows() {
    println!("cargo:rustc-link-arg-bins=/SUBSYSTEM:WINDOWS");
    println!("cargo:rustc-link-arg-bins=/ENTRY:mainCRTStartup");
    winresource::WindowsResource::new()
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
