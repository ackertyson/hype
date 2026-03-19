use crate::color::{alpha_blend, luminance, nearest_ansi256};

/// Pixel with RGBA.
#[derive(Clone, Copy)]
pub struct Pixel {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// Color output mode.
#[derive(Clone, Copy, PartialEq)]
pub enum ColorMode {
    True,
    Ansi256,
    Gray,
}

/// Dither method.
#[derive(Clone, Copy, PartialEq)]
pub enum Dither {
    None,
    FloydSteinberg,
    Ordered,
}

/// Render an image as half-block characters.
pub fn render_block(
    pixels: &[Pixel],
    width: usize,
    height: usize,
    color_mode: ColorMode,
    dither: Dither,
    bg: Option<(u8, u8, u8)>,
) -> String {
    let pixels = apply_bg(pixels, bg);
    let pixels = if color_mode == ColorMode::Ansi256 && dither != Dither::None {
        dither_pixels(&pixels, width, height, dither)
    } else {
        pixels
    };

    let mut out = String::new();
    let mut last_fg: Option<(u8, u8, u8)> = None;
    let mut last_bg_color: Option<(u8, u8, u8)> = None;

    let row_pairs = height / 2;
    for row in 0..row_pairs {
        let y_top = row * 2;
        let y_bot = y_top + 1;

        for x in 0..width {
            let top = pixels[y_top * width + x];
            let bot = pixels[y_bot * width + x];

            let top_opaque = top.a > 127;
            let bot_opaque = bot.a > 127;

            match (top_opaque, bot_opaque) {
                (true, true) => {
                    let fg = (top.r, top.g, top.b);
                    let bg_c = (bot.r, bot.g, bot.b);
                    emit_fg(&mut out, fg, &mut last_fg, color_mode);
                    emit_bg(&mut out, bg_c, &mut last_bg_color, color_mode);
                    out.push('▀');
                }
                (true, false) => {
                    let fg = (top.r, top.g, top.b);
                    // Reset BG if we had one
                    if last_bg_color.is_some() {
                        out.push_str("\x1b[49m");
                        last_bg_color = None;
                    }
                    emit_fg(&mut out, fg, &mut last_fg, color_mode);
                    out.push('▀');
                }
                (false, true) => {
                    let fg = (bot.r, bot.g, bot.b);
                    if last_bg_color.is_some() {
                        out.push_str("\x1b[49m");
                        last_bg_color = None;
                    }
                    emit_fg(&mut out, fg, &mut last_fg, color_mode);
                    out.push('▄');
                }
                (false, false) => {
                    if last_bg_color.is_some() {
                        out.push_str("\x1b[49m");
                        last_bg_color = None;
                    }
                    out.push(' ');
                }
            }
        }

        out.push_str("\x1b[0m");
        last_fg = None;
        last_bg_color = None;
        if row < row_pairs - 1 {
            out.push('\n');
        }
    }
    out
}

/// Render an image as braille characters.
pub fn render_braille(
    pixels: &[Pixel],
    width: usize,
    height: usize,
    color_mode: ColorMode,
    threshold: u8,
    bg: Option<(u8, u8, u8)>,
) -> String {
    let pixels = apply_bg(pixels, bg);
    let mut out = String::new();
    let mut last_fg: Option<(u8, u8, u8)> = None;

    // Braille dot mapping: each cell is 2 wide × 4 tall
    // Dot positions:
    //   0 3
    //   1 4
    //   2 5
    //   6 7
    let dot_bits: [u8; 8] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80];

    let cols = width / 2;
    let rows = height / 4;

    for row in 0..rows {
        for col in 0..cols {
            let px = col * 2;
            let py = row * 4;
            let mut pattern: u8 = 0;
            let mut r_sum: u32 = 0;
            let mut g_sum: u32 = 0;
            let mut b_sum: u32 = 0;
            let mut lit_count: u32 = 0;

            for dy in 0..4 {
                for dx in 0..2 {
                    let x = px + dx;
                    let y = py + dy;
                    if x < width && y < height {
                        let p = pixels[y * width + x];
                        let lum = luminance(p.r, p.g, p.b);
                        if lum > threshold as f32 {
                            let braille_idx = match (dx, dy) {
                                (0, 0) => 0,
                                (0, 1) => 1,
                                (0, 2) => 2,
                                (0, 3) => 6,
                                (1, 0) => 3,
                                (1, 1) => 4,
                                (1, 2) => 5,
                                (1, 3) => 7,
                                _ => unreachable!(),
                            };
                            pattern |= dot_bits[braille_idx];
                            r_sum += p.r as u32;
                            g_sum += p.g as u32;
                            b_sum += p.b as u32;
                            lit_count += 1;
                        }
                    }
                }
            }

            let ch = char::from_u32(0x2800 + pattern as u32).unwrap_or(' ');

            if lit_count > 0 && color_mode != ColorMode::Gray {
                let avg_r = (r_sum / lit_count) as u8;
                let avg_g = (g_sum / lit_count) as u8;
                let avg_b = (b_sum / lit_count) as u8;
                emit_fg(&mut out, (avg_r, avg_g, avg_b), &mut last_fg, color_mode);
            }
            out.push(ch);
        }

        out.push_str("\x1b[0m");
        last_fg = None;
        if row < rows - 1 {
            out.push('\n');
        }
    }
    out
}

/// Render an image as ASCII characters.
pub fn render_ascii(
    pixels: &[Pixel],
    width: usize,
    height: usize,
    color_mode: ColorMode,
    bg: Option<(u8, u8, u8)>,
) -> String {
    let pixels = apply_bg(pixels, bg);
    let ramp: &[u8] = b" .:-=+*%#@";
    let mut out = String::new();
    let mut last_fg: Option<(u8, u8, u8)> = None;

    for y in 0..height {
        for x in 0..width {
            let p = pixels[y * width + x];
            let lum = luminance(p.r, p.g, p.b);
            let idx = ((lum / 255.0) * (ramp.len() - 1) as f32) as usize;
            let ch = ramp[idx.min(ramp.len() - 1)] as char;

            if color_mode != ColorMode::Gray {
                emit_fg(&mut out, (p.r, p.g, p.b), &mut last_fg, color_mode);
            }
            out.push(ch);
        }
        out.push_str("\x1b[0m");
        last_fg = None;
        if y < height - 1 {
            out.push('\n');
        }
    }
    out
}

fn emit_fg(
    out: &mut String,
    color: (u8, u8, u8),
    last: &mut Option<(u8, u8, u8)>,
    mode: ColorMode,
) {
    if *last == Some(color) {
        return;
    }
    *last = Some(color);
    match mode {
        ColorMode::True => {
            out.push_str(&format!("\x1b[38;2;{};{};{}m", color.0, color.1, color.2));
        }
        ColorMode::Ansi256 => {
            let idx = nearest_ansi256(color.0, color.1, color.2);
            out.push_str(&format!("\x1b[38;5;{}m", idx));
        }
        ColorMode::Gray => {}
    }
}

fn emit_bg(
    out: &mut String,
    color: (u8, u8, u8),
    last: &mut Option<(u8, u8, u8)>,
    mode: ColorMode,
) {
    if *last == Some(color) {
        return;
    }
    *last = Some(color);
    match mode {
        ColorMode::True => {
            out.push_str(&format!("\x1b[48;2;{};{};{}m", color.0, color.1, color.2));
        }
        ColorMode::Ansi256 => {
            let idx = nearest_ansi256(color.0, color.1, color.2);
            out.push_str(&format!("\x1b[48;5;{}m", idx));
        }
        ColorMode::Gray => {}
    }
}

/// Apply background color to pixels with alpha, or pass through if no bg.
fn apply_bg(pixels: &[Pixel], bg: Option<(u8, u8, u8)>) -> Vec<Pixel> {
    match bg {
        Some((br, bg_g, bb)) => pixels
            .iter()
            .map(|p| {
                let (r, g, b) = alpha_blend(p.r, p.g, p.b, p.a, br, bg_g, bb);
                Pixel { r, g, b, a: 255 }
            })
            .collect(),
        None => pixels.to_vec(),
    }
}

/// Apply dithering to quantize colors for 256-color mode.
fn dither_pixels(pixels: &[Pixel], width: usize, height: usize, dither: Dither) -> Vec<Pixel> {
    match dither {
        Dither::FloydSteinberg => floyd_steinberg(pixels, width, height),
        Dither::Ordered => ordered_dither(pixels, width, height),
        Dither::None => pixels.to_vec(),
    }
}

fn floyd_steinberg(pixels: &[Pixel], width: usize, height: usize) -> Vec<Pixel> {
    let mut buf: Vec<[f32; 3]> = pixels
        .iter()
        .map(|p| [p.r as f32, p.g as f32, p.b as f32])
        .collect();

    let mut result = pixels.to_vec();

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            let old = buf[idx];
            let qr = quantize_channel(old[0]);
            let qg = quantize_channel(old[1]);
            let qb = quantize_channel(old[2]);

            result[idx].r = qr;
            result[idx].g = qg;
            result[idx].b = qb;

            let er = old[0] - qr as f32;
            let eg = old[1] - qg as f32;
            let eb = old[2] - qb as f32;

            let diffuse = |buf: &mut [[f32; 3]], i: usize, factor: f32| {
                buf[i][0] += er * factor;
                buf[i][1] += eg * factor;
                buf[i][2] += eb * factor;
            };

            if x + 1 < width {
                diffuse(&mut buf, idx + 1, 7.0 / 16.0);
            }
            if y + 1 < height {
                if x > 0 {
                    diffuse(&mut buf, idx + width - 1, 3.0 / 16.0);
                }
                diffuse(&mut buf, idx + width, 5.0 / 16.0);
                if x + 1 < width {
                    diffuse(&mut buf, idx + width + 1, 1.0 / 16.0);
                }
            }
        }
    }
    result
}

/// Quantize a float channel to the nearest 6-level cube value.
fn quantize_channel(v: f32) -> u8 {
    let v = v.clamp(0.0, 255.0) as u8;
    let levels: [u8; 6] = [0, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
    let mut best = levels[0];
    let mut best_dist = (v as i16 - best as i16).unsigned_abs();
    for &l in &levels[1..] {
        let d = (v as i16 - l as i16).unsigned_abs();
        if d < best_dist {
            best = l;
            best_dist = d;
        }
    }
    best
}

const BAYER_4X4: [[f32; 4]; 4] = [
    [0.0, 8.0, 2.0, 10.0],
    [12.0, 4.0, 14.0, 6.0],
    [3.0, 11.0, 1.0, 9.0],
    [15.0, 7.0, 13.0, 5.0],
];

fn ordered_dither(pixels: &[Pixel], width: usize, height: usize) -> Vec<Pixel> {
    let mut result = pixels.to_vec();
    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            let p = &pixels[idx];
            let threshold = (BAYER_4X4[y % 4][x % 4] / 16.0 - 0.5) * 64.0;
            result[idx].r = quantize_channel(p.r as f32 + threshold);
            result[idx].g = quantize_channel(p.g as f32 + threshold);
            result[idx].b = quantize_channel(p.b as f32 + threshold);
        }
    }
    result
}
