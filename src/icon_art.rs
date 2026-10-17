// The Wield logo, drawn procedurally so the window icon and the .exe icon (built by
// `build.rs`, which `include!`s this file) always match and need no binary assets.

/// Renders the logo as straight (non-premultiplied) RGBA, `n` x `n` pixels, 4x4 supersampled.
pub fn render_icon(n: usize) -> Vec<u8> {
    const SS: usize = 4;
    let scale = n as f32 / 64.0;
    // Bolt outline in 64-unit design space.
    let bolt = [(36.0, 6.0), (16.0, 36.0), (30.0, 36.0), (26.0, 58.0), (48.0, 26.0), (34.0, 26.0), (36.0, 6.0)];
    let inside_bolt = |x: f32, y: f32| {
        let mut inside = false;
        for w in bolt.windows(2) {
            let ((x1, y1), (x2, y2)): ((f32, f32), (f32, f32)) = (w[0], w[1]);
            if (y1 > y) != (y2 > y) && x < (x2 - x1) * (y - y1) / (y2 - y1) + x1 {
                inside = !inside;
            }
        }
        inside
    };
    let inside_tile = |x: f32, y: f32| {
        let r = 14.0;
        let cx = x.clamp(r, 64.0 - r);
        let cy = y.clamp(r, 64.0 - r);
        (x - cx).powi(2) + (y - cy).powi(2) <= r * r
    };

    let mut rgba = vec![0u8; n * n * 4];
    for py in 0..n {
        for px in 0..n {
            let (mut r, mut g, mut b, mut a) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) / scale;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / scale;
                    if !inside_tile(x, y) {
                        continue;
                    }
                    let t = (x + y) / 128.0;
                    // Gruvbox: yellow (#fabd2f) to orange (#fe8019) tile, bg0_h (#1d2021) bolt.
                    let (cr, cg, cb) = if inside_bolt(x, y) {
                        (29.0, 32.0, 33.0)
                    } else {
                        (250.0 + (254.0 - 250.0) * t, 189.0 + (128.0 - 189.0) * t, 47.0 + (25.0 - 47.0) * t)
                    };
                    r += cr;
                    g += cg;
                    b += cb;
                    a += 1.0;
                }
            }
            if a > 0.0 {
                let i = (py * n + px) * 4;
                rgba[i] = (r / a) as u8;
                rgba[i + 1] = (g / a) as u8;
                rgba[i + 2] = (b / a) as u8;
                rgba[i + 3] = (a / (SS * SS) as f32 * 255.0) as u8;
            }
        }
    }
    rgba
}

/// Builds a multi-size .ico from 32-bit BMP (DIB) entries.
#[allow(dead_code)]
pub fn ico_bytes(sizes: &[usize]) -> Vec<u8> {
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
    ico
}

#[cfg(test)]
mod tests {
    /// Regenerates installer/wield.ico. Run with `cargo test write_installer_icon -- --ignored`.
    #[test]
    #[ignore]
    fn write_installer_icon() {
        std::fs::write("installer/wield.ico", super::ico_bytes(&[16, 20, 24, 32, 40, 48, 64, 128, 256])).unwrap();
    }

    #[test]
    fn ico_has_all_sizes() {
        let ico = super::ico_bytes(&[16, 32, 256]);
        assert_eq!(&ico[..4], &[0, 0, 1, 0]);
        assert_eq!(u16::from_le_bytes([ico[4], ico[5]]), 3);
    }
}
