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
