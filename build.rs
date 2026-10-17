//! Embeds the application icon and version information into wield.exe.

use std::path::Path;

include!("src/icon_art.rs");

fn main() {
    println!("cargo:rerun-if-changed=src/icon_art.rs");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::env::var("OUT_DIR").unwrap();
    let ico = Path::new(&out).join("wield.ico");
    std::fs::write(&ico, ico_bytes(&[16, 20, 24, 32, 40, 48, 64, 128, 256])).expect("write icon");

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico.to_str().unwrap())
        .set("FileDescription", "Wield — Windows toolkit")
        .set("ProductName", "Wield")
        .set("LegalCopyright", "Copyright (c) 2026 Bitangsha Ray. MIT License.");
    res.compile().expect("embed Windows resources");
}
