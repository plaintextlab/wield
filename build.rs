//! Embeds the application icon and version information into wield.exe.

use std::path::Path;

include!("src/icon_art.rs");

/// Builds a multi-size .ico from 32-bit BMP (DIB) entries.
fn write_ico(path: &Path, sizes: &[usize]) {
    let images: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&n| {
            let rgba = render_icon(n);
            let mut dib = Vec::new();
            // BITMAPINFOHEADER; height is doubled to account for the AND mask.
            dib.extend_from_slice(&40u32.to_le_bytes());
            dib.extend_from_slice(&(n as i32).to_le_bytes());
            dib.extend_from_slice(&(2 * n as i32).to_le_bytes());
            dib.extend_from_slice(&1u16.to_le_bytes());
            dib.extend_from_slice(&32u16.to_le_bytes());
            dib.extend_from_slice(&[0u8; 24]);
            // XOR bitmap: BGRA, bottom-up.
            for y in (0..n).rev() {
                for x in 0..n {
                    let i = (y * n + x) * 4;
                    dib.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
                }
            }
            // AND mask: all zero (alpha channel is used), rows padded to 32 bits.
            let row = n.div_ceil(32) * 4;
            dib.extend(std::iter::repeat_n(0u8, row * n));
            dib
        })
        .collect();

    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len();
    for (&n, img) in sizes.iter().zip(&images) {
        let dim = if n >= 256 { 0 } else { n as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(img.len() as u32).to_le_bytes());
        ico.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += img.len();
    }
    for img in &images {
        ico.extend_from_slice(img);
    }
    std::fs::write(path, ico).expect("write icon");
}

fn main() {
    println!("cargo:rerun-if-changed=src/icon_art.rs");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::env::var("OUT_DIR").unwrap();
    let ico = Path::new(&out).join("wield.ico");
    write_ico(&ico, &[16, 20, 24, 32, 40, 48, 64, 128, 256]);

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico.to_str().unwrap())
        .set("FileDescription", "Wield — Windows toolkit")
        .set("ProductName", "Wield")
        .set("LegalCopyright", "Copyright (c) 2026 Bitangsha Ray. MIT License.");
    res.compile().expect("embed Windows resources");
}
