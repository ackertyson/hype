/// Perceptual luminance (BT.601).
pub fn luminance(r: u8, g: u8, b: u8) -> f32 {
    0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32
}

/// The 6×6×6 color cube levels.
pub const CUBE_LEVELS: [u8; 6] = [0, 0x5f, 0x87, 0xaf, 0xd7, 0xff];

/// Snap a single channel to the nearest cube level index (0–5).
pub fn nearest_cube_index(v: u8) -> usize {
    match v {
        0..=0x2f => 0,
        0x30..=0x72 => 1,
        0x73..=0x9b => 2,
        0x9c..=0xc3 => 3,
        0xc4..=0xeb => 4,
        _ => 5,
    }
}

/// Find the nearest ANSI 256-color index for an RGB value.
/// Checks the 6×6×6 cube (16–231) and the 24-step grayscale (232–255).
pub fn nearest_ansi256(r: u8, g: u8, b: u8) -> u8 {
    // Cube match
    let ri = nearest_cube_index(r);
    let gi = nearest_cube_index(g);
    let bi = nearest_cube_index(b);
    let cube_idx = 16 + (ri * 36 + gi * 6 + bi) as u8;
    let (cr, cg, cb) = (CUBE_LEVELS[ri], CUBE_LEVELS[gi], CUBE_LEVELS[bi]);
    let cube_dist = color_dist(r, g, b, cr, cg, cb);

    // Grayscale match
    let gray_avg = ((r as u16 + g as u16 + b as u16) / 3) as u8;
    let gray_step = if gray_avg < 8 {
        0
    } else {
        ((gray_avg - 8) / 10).min(23)
    };
    let gray_idx = 232 + gray_step;
    let gv = 8 + 10 * gray_step;
    let gray_dist = color_dist(r, g, b, gv, gv, gv);

    if gray_dist < cube_dist {
        gray_idx
    } else {
        cube_idx
    }
}

/// Weighted Euclidean distance (perceptual weighting).
fn color_dist(r1: u8, g1: u8, b1: u8, r2: u8, g2: u8, b2: u8) -> u32 {
    let dr = r1 as i32 - r2 as i32;
    let dg = g1 as i32 - g2 as i32;
    let db = b1 as i32 - b2 as i32;
    // Weight green more heavily (perceptual)
    (2 * dr * dr + 4 * dg * dg + 3 * db * db) as u32
}

/// Alpha-blend a foreground pixel over a background color.
/// Standard "over" compositing.
pub fn alpha_blend(fr: u8, fg: u8, fb: u8, fa: u8, br: u8, bg: u8, bb: u8) -> (u8, u8, u8) {
    if fa == 255 {
        return (fr, fg, fb);
    }
    if fa == 0 {
        return (br, bg, bb);
    }
    let a = fa as f32 / 255.0;
    let inv = 1.0 - a;
    let r = (fr as f32 * a + br as f32 * inv) as u8;
    let g = (fg as f32 * a + bg as f32 * inv) as u8;
    let b = (fb as f32 * a + bb as f32 * inv) as u8;
    (r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_ansi256_pure_red() {
        // Pure red (255,0,0) → cube index 16 + 5*36 = 196
        assert_eq!(nearest_ansi256(255, 0, 0), 196);
    }

    #[test]
    fn nearest_ansi256_pure_green() {
        // Pure green (0,255,0) → cube index 16 + 5*6 = 46
        assert_eq!(nearest_ansi256(0, 255, 0), 46);
    }

    #[test]
    fn nearest_ansi256_pure_blue() {
        // Pure blue (0,0,255) → cube index 16 + 5 = 21
        assert_eq!(nearest_ansi256(0, 0, 255), 21);
    }

    #[test]
    fn nearest_ansi256_white() {
        // Pure white (255,255,255) → cube 16+5*36+5*6+5=231 or grayscale 255=231
        let idx = nearest_ansi256(255, 255, 255);
        // Should pick cube 231 or grayscale — both are valid white
        assert!(idx == 231 || idx == 255);
    }

    #[test]
    fn nearest_ansi256_mid_gray_prefers_grayscale() {
        // A neutral gray like (128,128,128) should prefer the grayscale ramp
        // over the color cube, since grayscale has finer steps for grays
        let idx = nearest_ansi256(128, 128, 128);
        assert!(idx >= 232, "mid gray should use grayscale ramp, got {idx}");
    }

    #[test]
    fn nearest_ansi256_near_cube_prefers_cube() {
        // A saturated color like (0x5f, 0, 0xaf) should match the cube exactly
        // Cube: ri=1, gi=0, bi=3 → 16 + 1*36 + 0*6 + 3 = 55
        assert_eq!(nearest_ansi256(0x5f, 0, 0xaf), 55);
    }

    #[test]
    fn alpha_blend_fully_opaque() {
        assert_eq!(alpha_blend(100, 150, 200, 255, 0, 0, 0), (100, 150, 200));
    }

    #[test]
    fn alpha_blend_fully_transparent() {
        assert_eq!(alpha_blend(100, 150, 200, 0, 10, 20, 30), (10, 20, 30));
    }

    #[test]
    fn alpha_blend_half() {
        let (r, g, b) = alpha_blend(200, 100, 0, 128, 0, 0, 0);
        // ~50% blend: 200*0.502 + 0*0.498 ≈ 100
        assert!((r as i16 - 100).unsigned_abs() <= 2, "r={r}");
        assert!((g as i16 - 50).unsigned_abs() <= 2, "g={g}");
        assert_eq!(b, 0);
    }

    #[test]
    fn alpha_blend_over_white() {
        let (r, g, b) = alpha_blend(0, 0, 0, 128, 255, 255, 255);
        // ~50% black over white ≈ 127
        assert!((r as i16 - 127).unsigned_abs() <= 2, "r={r}");
        assert!((g as i16 - 127).unsigned_abs() <= 2, "g={g}");
        assert!((b as i16 - 127).unsigned_abs() <= 2, "b={b}");
    }
}
