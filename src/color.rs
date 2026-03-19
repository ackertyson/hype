/// Perceptual luminance (BT.601).
pub fn luminance(r: u8, g: u8, b: u8) -> f32 {
    0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32
}

/// The 6×6×6 color cube levels.
const CUBE_LEVELS: [u8; 6] = [0, 0x5f, 0x87, 0xaf, 0xd7, 0xff];

/// Snap a single channel to the nearest cube level index (0–5).
fn nearest_cube_index(v: u8) -> usize {
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
