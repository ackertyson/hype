use crate::color::{
    CUBE_LEVELS, alpha_blend, ansi256_rgb, color_dist, lum1000, nearest_ansi256, nearest_cube_index,
};

/// Pixel with RGBA.
#[derive(Clone, Copy)]
pub struct Pixel {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// Color output mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColorMode {
    True,
    Ansi256,
    Gray,
}

/// Dither method.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dither {
    None,
    FloydSteinberg,
    Ordered,
}

/// What the output is shown over.
#[derive(Clone, Copy)]
pub struct Backdrop {
    /// Flatten transparency onto this color (`--bg`).
    pub bg: Option<(u8, u8, u8)>,
    /// The background is light (`--light`: a light terminal theme, or
    /// paper) rather than the usual dark.
    pub light: bool,
}

impl Backdrop {
    /// The assumed background color.
    pub(crate) fn paper(self) -> Rgb {
        if self.light {
            (255, 255, 255)
        } else {
            (0, 0, 0)
        }
    }
}

/// No color set: the terminal default.
const DEFAULT: u32 = u32::MAX;

/// Tracks the current SGR state and emits only what changes, combining
/// foreground and background into a single escape sequence.
struct Sgr {
    mode: ColorMode,
    /// Colors are given inverted (see `flatten`) and emitted as their inverse.
    invert: bool,
    fg: u32,
    bg: u32,
}

impl Sgr {
    fn new(mode: ColorMode) -> Self {
        Sgr {
            mode,
            invert: false,
            fg: DEFAULT,
            bg: DEFAULT,
        }
    }

    /// Reduce a color to what is actually emitted, so colors that map to the
    /// same 256-color index are not re-sent.
    fn key(&self, (r, g, b): (u8, u8, u8)) -> u32 {
        let (r, g, b) = if self.invert {
            (255 - r, 255 - g, 255 - b)
        } else {
            (r, g, b)
        };
        match self.mode {
            ColorMode::True => (r as u32) << 16 | (g as u32) << 8 | b as u32,
            ColorMode::Ansi256 => nearest_ansi256(r, g, b) as u32,
            ColorMode::Gray => {
                let l = (lum1000(r, g, b) / 1000) as u8;
                nearest_ansi256(l, l, l) as u32
            }
        }
    }

    /// The color the terminal will actually display for `c`.
    fn shown(&self, c: Rgb) -> Rgb {
        match self.mode {
            ColorMode::True => c,
            _ => ansi256_rgb(self.key(c) as u8),
        }
    }

    /// Switch to `fg` (if given) and `bg` (`DEFAULT` for terminal default).
    fn set(&mut self, out: &mut Vec<u8>, fg: Option<u32>, bg: u32) {
        let fg = fg.filter(|&f| f != self.fg);
        let bg = (bg != self.bg).then_some(bg);
        if fg.is_none() && bg.is_none() {
            return;
        }
        out.extend_from_slice(b"\x1b[");
        if let Some(f) = fg {
            self.param(out, b"38", f);
            self.fg = f;
        }
        if let Some(b) = bg {
            if fg.is_some() {
                out.push(b';');
            }
            self.param(out, b"48", b);
            self.bg = b;
        }
        out.push(b'm');
    }

    fn param(&self, out: &mut Vec<u8>, layer: &[u8], key: u32) {
        if key == DEFAULT {
            out.extend_from_slice(if layer == b"38" { b"39" } else { b"49" });
            return;
        }
        out.extend_from_slice(layer);
        if self.mode == ColorMode::True {
            out.extend_from_slice(b";2;");
            push_dec(out, (key >> 16) as u8);
            out.push(b';');
            push_dec(out, (key >> 8) as u8);
            out.push(b';');
            push_dec(out, key as u8);
        } else {
            out.extend_from_slice(b";5;");
            push_dec(out, key as u8);
        }
    }

    /// Reset at end of line so each line stands alone and no background
    /// color bleeds past the image edge.
    fn end_line(&mut self, out: &mut Vec<u8>) {
        if self.fg != DEFAULT || self.bg != DEFAULT {
            out.extend_from_slice(b"\x1b[0m");
        }
        self.fg = DEFAULT;
        self.bg = DEFAULT;
    }
}

/// Decimal digits for 0–255, precomputed: (bytes, len).
const DEC: [([u8; 3], u8); 256] = {
    let mut t = [([0u8; 3], 0u8); 256];
    let mut i = 0;
    while i < 256 {
        let v = i as u8;
        t[i] = if v >= 100 {
            ([b'0' + v / 100, b'0' + v / 10 % 10, b'0' + v % 10], 3)
        } else if v >= 10 {
            ([b'0' + v / 10, b'0' + v % 10, 0], 2)
        } else {
            ([b'0' + v, 0, 0], 1)
        };
        i += 1;
    }
    t
};

fn push_dec(out: &mut Vec<u8>, v: u8) {
    let (d, n) = &DEC[v as usize];
    out.extend_from_slice(&d[..*n as usize]);
}

fn push_char(out: &mut Vec<u8>, c: char) {
    let mut b = [0; 4];
    out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
}

fn newline(out: &mut Vec<u8>, row: usize, rows: usize) {
    if row + 1 < rows {
        out.push(b'\n');
    }
}

/// Every renderer samples each character cell as 2×4 subpixels, indexed
/// `y * 2 + x`, and uses them to pick the glyph that best fits the cell.
pub(crate) fn cell(pixels: &[Pixel], width: usize, col: usize, row: usize) -> [Pixel; 8] {
    std::array::from_fn(|i| pixels[(row * 4 + i / 2) * width + col * 2 + i % 2])
}

/// Two-region block glyphs as subpixel masks: (mask, glyph drawing the mask
/// in the foreground, glyph drawing its complement in the foreground).
const SHAPES: [(u8, char, char); 7] = [
    (0x0f, '▀', '▄'),
    (0x55, '▌', '▐'),
    (0x05, '▘', '▟'),
    (0x0a, '▝', '▙'),
    (0x50, '▖', '▜'),
    (0xa0, '▗', '▛'),
    (0xa5, '▚', '▞'),
];

pub(crate) type Rgb = (u8, u8, u8);

/// Mean color of the subpixels in `mask` (which must be non-empty).
pub(crate) fn mean(sub: &[Pixel; 8], mask: u8) -> Rgb {
    let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
    for (i, p) in sub.iter().enumerate() {
        if mask >> i & 1 == 1 {
            (r, g, b, n) = (r + p.r as u32, g + p.g as u32, b + p.b as u32, n + 1);
        }
    }
    let q = |v: u32| ((v + n / 2) / n) as u8;
    (q(r), q(g), q(b))
}

/// Best two-color shape for a fully opaque cell: each side gets its mean
/// color as the terminal will actually show it (`shown`), and the shape with
/// the least perceptual error over the subpixels wins.
fn fit(sub: &[Pixel; 8], shown: impl Fn(Rgb) -> Rgb) -> (char, char, Rgb, Rgb) {
    SHAPES
        .iter()
        .map(|&(mask, g1, g0)| {
            let (a, b) = (shown(mean(sub, mask)), shown(mean(sub, !mask)));
            let err: u32 = sub
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let c = if mask >> i & 1 == 1 { a } else { b };
                    color_dist(p.r, p.g, p.b, c.0, c.1, c.2)
                })
                .sum();
            (err, g1, g0, a, b)
        })
        .min_by_key(|t| t.0)
        .map(|(_, g1, g0, a, b)| (g1, g0, a, b))
        .unwrap()
}

/// Emit a two-color cell: `g1` draws `a` in the foreground over `b`, and
/// `g0` is the same picture with the colors swapped. Pick whichever needs
/// fewer color changes; a uniform cell is a background-colored space.
fn two_color(out: &mut Vec<u8>, sgr: &mut Sgr, g1: char, g0: char, a: u32, b: u32) {
    if a == b {
        sgr.set(out, None, a);
        out.push(b' ');
        return;
    }
    let keep = (sgr.fg != a) as u8 + (sgr.bg != b) as u8;
    let swap = (sgr.fg != b) as u8 + (sgr.bg != a) as u8;
    if swap < keep {
        sgr.set(out, Some(b), a);
        push_char(out, g0);
    } else {
        sgr.set(out, Some(a), b);
        push_char(out, g1);
    }
}

/// Render an image as block characters: per cell, the half-block or
/// quadrant shape and two colors that best fit its subpixels.
pub fn render_block(
    pixels: &mut [Pixel],
    width: usize,
    height: usize,
    color_mode: ColorMode,
    dither: Dither,
    backdrop: Backdrop,
    out: &mut Vec<u8>,
) {
    match backdrop.bg {
        Some(_) => apply_bg(pixels, backdrop.bg),
        // Transparent areas show the terminal background, so fade translucent
        // edges toward it (as assumed in braille and ascii) to keep
        // silhouettes antialiased rather than ending in a full-color rim.
        None => {
            let (br, bg, bb) = backdrop.paper();
            for p in pixels.iter_mut() {
                (p.r, p.g, p.b) = alpha_blend(p.r, p.g, p.b, p.a, br, bg, bb);
            }
        }
    }
    if color_mode == ColorMode::Ansi256 && dither != Dither::None {
        return render_dithered(pixels, width, height, dither, out);
    }

    let mut sgr = Sgr::new(color_mode);
    let rows = height / 4;
    for row in 0..rows {
        for col in 0..width / 2 {
            let sub = cell(pixels, width, col, row);
            let opaque = (0..8).fold(0u8, |m, i| m | ((sub[i].a > 127) as u8) << i);
            match opaque {
                0xff => {
                    let (g1, g0, a, b) = fit(&sub, |c| sgr.shown(c));
                    let (a, b) = (sgr.key(a), sgr.key(b));
                    two_color(out, &mut sgr, g1, g0, a, b);
                }
                0 => {
                    sgr.set(out, None, DEFAULT);
                    out.push(b' ');
                }
                _ => {
                    // Partly transparent: draw the shape closest to the opaque
                    // subpixels in their mean color over the terminal default.
                    let (_, mask, glyph) = SHAPES
                        .iter()
                        .flat_map(|&(m, g1, g0)| [(m, g1), (!m, g0)])
                        .map(|(m, g)| ((m ^ opaque).count_ones(), m, g))
                        .min_by_key(|&(d, _, _)| d)
                        .unwrap();
                    let ink = if mask & opaque != 0 {
                        mask & opaque
                    } else {
                        opaque
                    };
                    let c = mean(&sub, ink);
                    sgr.set(out, Some(sgr.key(c)), DEFAULT);
                    push_char(out, glyph);
                }
            }
        }
        sgr.end_line(out);
        newline(out, row, rows);
    }
}

/// 256-color dithering works on whole pixels, so it uses plain half blocks:
/// each 2×4 cell is averaged down to a top and bottom pixel first.
fn render_dithered(
    pixels: &[Pixel],
    width: usize,
    height: usize,
    dither: Dither,
    out: &mut Vec<u8>,
) {
    let (cols, rows) = (width / 2, height / 4);
    let mut halves: Vec<Pixel> = (0..rows * 2)
        .flat_map(|y| (0..cols).map(move |x| (x, y)))
        .map(|(x, y)| {
            let sub = cell(pixels, width, x, y / 2);
            let half = if y % 2 == 0 { 0x0f } else { 0xf0 };
            let (r, g, b) = mean(&sub, half);
            let a = (0..8)
                .filter(|i| half >> i & 1 == 1)
                .map(|i| sub[i].a as u32)
                .sum::<u32>()
                / 4;
            Pixel {
                r,
                g,
                b,
                a: a as u8,
            }
        })
        .collect();
    match dither {
        Dither::FloydSteinberg => floyd_steinberg(&mut halves, cols, rows * 2),
        Dither::Ordered => ordered_dither(&mut halves, cols, rows * 2),
        Dither::None => {}
    }

    let mut sgr = Sgr::new(ColorMode::Ansi256);
    for row in 0..rows {
        let (top_row, bot_row) = halves[row * 2 * cols..][..cols * 2].split_at(cols);
        for (top, bot) in top_row.iter().zip(bot_row) {
            let t = (top.a > 127).then(|| sgr.key((top.r, top.g, top.b)));
            let b = (bot.a > 127).then(|| sgr.key((bot.r, bot.g, bot.b)));
            match (t, b) {
                (Some(t), Some(b)) => two_color(out, &mut sgr, '▀', '▄', t, b),
                (Some(t), None) => {
                    sgr.set(out, Some(t), DEFAULT);
                    push_char(out, '▀');
                }
                (None, Some(b)) => {
                    sgr.set(out, Some(b), DEFAULT);
                    push_char(out, '▄');
                }
                (None, None) => {
                    sgr.set(out, None, DEFAULT);
                    out.push(b' ');
                }
            }
        }
        sgr.end_line(out);
        newline(out, row, rows);
    }
}

/// Braille and ASCII draw one color per cell over the terminal background,
/// and their logic assumes it is black. So first show translucent pixels as
/// they would look over the background (`--bg`, else the assumed one). On a
/// light background, then invert the image: what the dark-background logic
/// sees as brightness is then ink, and `Sgr` inverts the colors back on
/// output. Every decision carries over exactly, as `color_dist` depends only
/// on channel differences and `lum1000` is linear.
pub(crate) fn flatten(pixels: &mut [Pixel], backdrop: Backdrop) {
    let (br, bg, bb) = backdrop.bg.unwrap_or(backdrop.paper());
    for p in pixels {
        let (r, g, b) = alpha_blend(p.r, p.g, p.b, p.a, br, bg, bb);
        *p = if backdrop.light {
            Pixel {
                r: 255 - r,
                g: 255 - g,
                b: 255 - b,
                a: 255,
            }
        } else {
            Pixel { r, g, b, a: 255 }
        };
    }
}

/// The SGR state for a mode drawn over the `flatten`ed image.
fn flat_sgr(color_mode: ColorMode, backdrop: Backdrop) -> Sgr {
    Sgr {
        invert: backdrop.light,
        ..Sgr::new(color_mode)
    }
}

/// Which subpixels to draw in a cell's single foreground color, leaving the
/// rest to the background (black, after `flatten`). Starting from those
/// brighter than `limit`, a subpixel stays lit only if the lit subpixels'
/// mean color represents it better than black does.
pub(crate) fn lit_mask(sub: &[Pixel; 8], limit: u32) -> u8 {
    let bright = (0..8).fold(0u8, |m, i| {
        let p = sub[i];
        m | ((lum1000(p.r, p.g, p.b) > limit) as u8) << i
    });
    let mut mask = bright;
    for _ in 0..2 {
        if mask == 0 {
            break;
        }
        let c = mean(sub, mask);
        mask = (0..8).fold(0u8, |m, i| {
            let p = sub[i];
            let closer =
                color_dist(p.r, p.g, p.b, c.0, c.1, c.2) < color_dist(p.r, p.g, p.b, 0, 0, 0);
            m | (closer as u8) << i
        }) & bright;
    }
    mask
}

/// Render an image as braille characters.
pub fn render_braille(
    pixels: &mut [Pixel],
    width: usize,
    height: usize,
    color_mode: ColorMode,
    threshold: u8,
    backdrop: Backdrop,
    out: &mut Vec<u8>,
) {
    flatten(pixels, backdrop);
    let mut sgr = flat_sgr(color_mode, backdrop);
    let limit = threshold as u32 * 1000;

    // Braille dot bit for each subpixel index y * 2 + x:
    //   0 3
    //   1 4
    //   2 5
    //   6 7
    const DOT: [u8; 8] = [0x01, 0x08, 0x02, 0x10, 0x04, 0x20, 0x40, 0x80];

    let rows = height / 4;
    for row in 0..rows {
        for col in 0..width / 2 {
            let sub = cell(pixels, width, col, row);
            let mask = lit_mask(&sub, limit);
            let pattern = (0..8)
                .filter(|i| mask >> i & 1 == 1)
                .fold(0u8, |pat, i| pat | DOT[i]);

            // Blank cells need no color.
            if mask != 0 && color_mode != ColorMode::Gray {
                let k = sgr.key(mean(&sub, mask));
                sgr.set(out, Some(k), DEFAULT);
            }
            // U+2800 + pattern, UTF-8 encoded directly.
            let cp = 0x2800 | pattern as u32;
            out.extend_from_slice(&[
                0xe0 | (cp >> 12) as u8,
                0x80 | ((cp >> 6) & 0x3f) as u8,
                0x80 | (cp & 0x3f) as u8,
            ]);
        }
        sgr.end_line(out);
        newline(out, row, rows);
    }
}

/// ASCII density ramp, darkest to brightest, with each glyph's ink coverage (0–255) over a cell's 2×4 subpixels,
/// measured from Menlo, SF Mono and Monaco and averaged, then scaled so the
/// densest glyph reads as full brightness.
const GLYPHS: [(u8, [u8; 8]); 69] = [
    (b' ', [0, 0, 0, 0, 0, 0, 0, 0]),
    (b'.', [0, 0, 0, 0, 98, 72, 53, 47]),
    (b'\'', [47, 54, 117, 105, 0, 0, 0, 0]),
    (b'`', [98, 22, 5, 36, 0, 0, 0, 0]),
    (b'^', [36, 39, 238, 235, 46, 46, 0, 0]),
    (b'"', [81, 79, 200, 210, 0, 0, 0, 0]),
    (b',', [0, 0, 0, 0, 73, 58, 194, 86]),
    (b':', [0, 0, 124, 111, 76, 65, 50, 47]),
    (b';', [0, 0, 129, 106, 73, 58, 184, 85]),
    (b'I', [138, 139, 154, 167, 166, 178, 121, 122]),
    (b'l', [178, 42, 143, 127, 136, 165, 60, 128]),
    (b'!', [31, 43, 127, 164, 80, 100, 39, 47]),
    (b'i', [53, 67, 173, 105, 109, 188, 91, 136]),
    (b'>', [0, 0, 190, 78, 208, 251, 20, 0]),
    (b'<', [0, 0, 79, 188, 250, 210, 0, 20]),
    (b'~', [0, 0, 101, 10, 174, 251, 0, 0]),
    (b'+', [0, 0, 82, 108, 255, 255, 10, 11]),
    (b'_', [0, 0, 0, 0, 0, 0, 214, 212]),
    (b'-', [0, 0, 0, 0, 181, 183, 0, 0]),
    (b'?', [108, 102, 79, 255, 145, 63, 61, 21]),
    (b']', [150, 117, 40, 230, 40, 230, 144, 160]),
    (b'[', [95, 169, 196, 73, 196, 73, 143, 158]),
    (b'}', [179, 28, 152, 143, 128, 213, 212, 47]),
    (b'{', [21, 182, 137, 159, 206, 137, 41, 214]),
    (b'1', [62, 55, 187, 225, 81, 252, 124, 154]),
    (b')', [131, 13, 34, 255, 30, 252, 136, 56]),
    (b'(', [15, 128, 255, 31, 255, 23, 54, 136]),
    (b'|', [32, 56, 96, 154, 96, 154, 62, 111]),
    (b'/', [0, 83, 19, 246, 252, 19, 98, 0]),
    (b't', [31, 1, 255, 162, 255, 43, 28, 131]),
    (b'f', [27, 188, 255, 215, 234, 37, 49, 6]),
    (b'j', [2, 112, 141, 189, 4, 255, 179, 201]),
    (b'r', [0, 0, 231, 237, 255, 5, 83, 29]),
    (b'x', [0, 0, 193, 179, 255, 255, 62, 65]),
    (b'n', [0, 0, 255, 238, 255, 255, 55, 54]),
    (b'u', [0, 0, 164, 161, 255, 255, 115, 103]),
    (b'v', [0, 0, 173, 164, 255, 255, 39, 39]),
    (b'c', [0, 0, 204, 185, 255, 54, 69, 134]),
    (b'z', [0, 0, 161, 255, 241, 132, 135, 131]),
    (b'X', [78, 72, 255, 255, 255, 255, 62, 65]),
    (b'Y', [77, 73, 255, 255, 144, 158, 27, 30]),
    (b'U', [72, 68, 255, 255, 255, 255, 103, 99]),
    (b'J', [77, 121, 12, 255, 69, 255, 142, 64]),
    (b'C', [80, 149, 255, 58, 255, 65, 67, 136]),
    (b'L', [71, 0, 255, 0, 255, 42, 118, 142]),
    (b'Q', [110, 111, 255, 255, 255, 255, 93, 255]),
    (b'0', [100, 99, 255, 255, 255, 255, 93, 92]),
    (b'O', [110, 111, 255, 255, 255, 255, 94, 97]),
    (b'Z', [148, 168, 49, 255, 255, 66, 150, 150]),
    (b'm', [0, 0, 255, 255, 255, 255, 74, 77]),
    (b'w', [0, 0, 225, 219, 255, 255, 64, 63]),
    (b'q', [0, 0, 228, 255, 255, 255, 110, 255]),
    (b'p', [0, 0, 255, 241, 255, 255, 255, 92]),
    (b'd', [0, 91, 229, 255, 255, 255, 111, 105]),
    (b'b', [94, 0, 255, 242, 255, 255, 131, 92]),
    (b'k', [95, 0, 255, 195, 255, 255, 55, 69]),
    (b'h', [93, 0, 255, 239, 255, 255, 55, 54]),
    (b'a', [0, 0, 185, 231, 255, 255, 121, 97]),
    (b'o', [0, 0, 230, 230, 255, 255, 95, 98]),
    (b'*', [19, 21, 255, 255, 203, 194, 7, 5]),
    (b'#', [35, 63, 255, 255, 255, 255, 55, 37]),
    (b'M', [94, 91, 255, 255, 255, 255, 52, 54]),
    (b'W', [81, 77, 255, 255, 255, 255, 65, 64]),
    (b'&', [116, 60, 255, 134, 255, 255, 134, 131]),
    (b'8', [107, 118, 255, 255, 255, 255, 104, 108]),
    (b'%', [106, 28, 255, 242, 237, 255, 34, 108]),
    (b'B', [151, 94, 255, 255, 255, 255, 137, 87]),
    (b'@', [73, 89, 255, 255, 255, 255, 165, 126]),
    (b'$', [66, 107, 255, 240, 172, 255, 120, 136]),
];

/// Render an image as ASCII characters: the density ramp in flat areas, and
/// at edges the glyph whose shape best matches the cell's subpixel brightness.
pub fn render_ascii(
    pixels: &mut [Pixel],
    width: usize,
    height: usize,
    color_mode: ColorMode,
    backdrop: Backdrop,
    out: &mut Vec<u8>,
) {
    flatten(pixels, backdrop);
    let mut sgr = flat_sgr(color_mode, backdrop);

    let rows = height / 4;
    for row in 0..rows {
        for col in 0..width / 2 {
            let sub = cell(pixels, width, col, row);
            let &(ch, ink) = ascii_glyph(&sub, &GLYPHS);
            if color_mode != ColorMode::Gray && ch != b' ' {
                let k = sgr.key(ink_mean(&sub, &ink));
                sgr.set(out, Some(k), DEFAULT);
            }
            out.push(ch);
        }
        sgr.end_line(out);
        newline(out, row, rows);
    }
}

/// A glyph and its ink coverage over a cell's 2×4 subpixels.
type Glyph = (u8, [u8; 8]);

/// ASCII mode's glyph for a cell, from a density `ramp` like `GLYPHS`: the
/// first entry is blank, the rest by increasing mean coverage. Entries are
/// (label, coverage); only the coverage is read, so a label can carry more
/// than the character (svg.rs adds the font weight).
pub(crate) fn ascii_glyph<'a, L>(sub: &[Pixel; 8], ramp: &'a [(L, [u8; 8])]) -> &'a (L, [u8; 8]) {
    let lum = sub.map(|p| lum1000(p.r, p.g, p.b));
    // An edge is a cell the dark-background fit splits in two.
    let mask = lit_mask(sub, 0);
    if mask != 0 && mask != 0xff {
        // Edges get the glyph whose shape best matches the cell.
        ramp.iter()
            .min_by_key(|(_, cov)| {
                (0..8)
                    .map(|i| (lum[i] as i32 / 1000 - cov[i] as i32).pow(2))
                    .sum::<i32>()
            })
            .unwrap()
    } else {
        // Flat areas use the density ramp on the cell's mean luminance.
        let mean = lum.iter().sum::<u32>() / 8;
        &ramp[(mean * (ramp.len() as u32 - 1) / 255_000) as usize]
    }
}

/// The cell's color under a glyph's ink, weighting subpixels by coverage.
/// The glyph must have some ink.
pub(crate) fn ink_mean(sub: &[Pixel; 8], ink: &[u8; 8]) -> Rgb {
    let w: u32 = ink.iter().map(|&c| c as u32).sum();
    let avg = |f: fn(&Pixel) -> u8| {
        let s: u32 = (0..8).map(|i| f(&sub[i]) as u32 * ink[i] as u32).sum();
        (s / w) as u8
    };
    (avg(|p| p.r), avg(|p| p.g), avg(|p| p.b))
}

/// Marks an italic character in `prepare_text`'s output (bit 7: the text is
/// otherwise printable ASCII).
pub const ITALIC: u8 = 0x80;

/// Text for `render_text`: whitespace runs collapse to one space (also
/// joining the end back to the start, as the text repeats), typographic
/// quotes, dashes and ellipses become their ASCII forms, and anything else
/// outside printable ASCII is dropped, so every character is one column.
/// Underscores marking italics, as in Project Gutenberg's `_word_`, are
/// dropped and the characters between them marked `ITALIC`. None if no
/// printable character is left.
pub fn prepare_text(bytes: &[u8]) -> Option<Vec<u8>> {
    let chars: Vec<char> = String::from_utf8_lossy(bytes).chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    let (mut space, mut italic, mut newlines) = (false, false, 0);
    let word = |c: Option<&char>| c.is_some_and(|c| c.is_alphanumeric());
    let blank = |c: Option<&char>| c.is_none_or(|c| c.is_whitespace());
    for (i, &c) in chars.iter().enumerate() {
        // A paragraph break ends italics, so a stray underscore cannot run
        // on through the rest of the text.
        if c == '\n' {
            newlines += 1;
            italic &= newlines < 2;
        } else if !c.is_whitespace() {
            newlines = 0;
        }
        if c == '_' {
            // Markup opens at the start of a word and closes at the end of
            // one; any other underscore (snake_case, a lone _) is literal.
            let (prev, next) = (
                i.checked_sub(1).and_then(|j| chars.get(j)),
                chars.get(i + 1),
            );
            if !italic && !word(prev) && !blank(next) || italic && !blank(prev) && !word(next) {
                italic = !italic;
                continue;
            }
        }
        let ascii: &[u8] = match c {
            '\u{2018}' | '\u{2019}' => b"'",
            '\u{201c}' | '\u{201d}' => b"\"",
            '\u{2013}' | '\u{2014}' => b"-",
            '\u{2026}' => b"...",
            c if c.is_whitespace() => {
                space = true;
                continue;
            }
            '!'..='~' => &[c as u8],
            _ => continue,
        };
        let flag = if italic { ITALIC } else { 0 };
        if let Some(&last) = out.last().filter(|_| space) {
            // Italic only between italic words.
            out.push(b' ' | (flag & last));
        }
        space = false;
        out.extend(ascii.iter().map(|&b| b | flag));
    }
    if out.is_empty() {
        return None;
    }
    out.push(b' ');
    Some(out)
}

/// Total ink of an average lowercase letter in `table`: text is shown as
/// bright as such a letter would be in the cell's color.
const fn ref_ink(table: &[Glyph]) -> u32 {
    let (mut sum, mut n, mut i) = (0, 0, 0);
    while i < table.len() {
        if table[i].0.is_ascii_lowercase() {
            let mut j = 0;
            while j < 8 {
                sum += table[i].1[j] as u32;
                j += 1;
            }
            n += 1;
        }
        i += 1;
    }
    sum / n
}

const REF_INK: u32 = ref_ink(&GLYPHS);

/// Scale `c` so a glyph with `ink` looks as bright as one with `ref_ink`
/// would: thin glyphs are brightened and heavy ones dimmed. Brightening stops
/// where the largest channel reaches 255, so the hue is kept. On a light
/// background `c` is inverted (`flatten`), so thin glyphs get darker ink.
pub(crate) fn compensate((r, g, b): Rgb, ink: &[u8; 8], ref_ink: u32) -> Rgb {
    let cov: u32 = ink.iter().map(|&c| c as u32).sum();
    let max = r.max(g).max(b) as u32;
    let (num, den) = if max * ref_ink > 255 * cov {
        (255, max)
    } else {
        (ref_ink, cov)
    };
    let s = |v: u8| (v as u32 * num / den) as u8;
    (s(r), s(g), s(b))
}

/// Render an image as the given text (from `prepare_text`): each cell that
/// ASCII mode would not leave blank takes the next character, in reading
/// order, colored by the image under its ink and brightness-compensated for
/// its coverage. The text repeats if the image outlasts it.
pub fn render_text(
    pixels: &mut [Pixel],
    width: usize,
    height: usize,
    color_mode: ColorMode,
    backdrop: Backdrop,
    text: &[u8],
    out: &mut Vec<u8>,
) {
    flatten(pixels, backdrop);
    let mut sgr = flat_sgr(color_mode, backdrop);
    let mut text = text.iter().copied().cycle();

    let rows = height / 4;
    for row in 0..rows {
        for col in 0..width / 2 {
            let sub = cell(pixels, width, col, row);
            if ascii_glyph(&sub, &GLYPHS).0 == b' ' {
                out.push(b' ');
                continue;
            }
            // The terminal draws italics upright.
            let ch = text.next().unwrap_or(b' ') & !ITALIC;
            if color_mode != ColorMode::Gray && ch != b' ' {
                // Glyphs not in the measured table get the plain cell mean,
                // unscaled: about right for a letter, as REF_INK is.
                let c = match GLYPHS.iter().find(|(g, _)| *g == ch) {
                    Some((_, ink)) => compensate(ink_mean(&sub, ink), ink, REF_INK),
                    None => mean(&sub, 0xff),
                };
                let k = sgr.key(c);
                sgr.set(out, Some(k), DEFAULT);
            }
            out.push(ch);
        }
        sgr.end_line(out);
        newline(out, row, rows);
    }
}

/// Composite pixels over a background color in place.
fn apply_bg(pixels: &mut [Pixel], bg: Option<(u8, u8, u8)>) {
    if let Some((br, bgc, bb)) = bg {
        for p in pixels {
            (p.r, p.g, p.b) = alpha_blend(p.r, p.g, p.b, p.a, br, bgc, bb);
            p.a = 255;
        }
    }
}

/// Composite `layer` (`layer_w` wide) over `canvas` (`canvas_w` wide) with
/// its top-left corner at (`x0`, `y0`), using straight-alpha "over". The
/// layer must lie within the canvas.
pub fn overlay(
    canvas: &mut [Pixel],
    canvas_w: usize,
    layer: &[Pixel],
    layer_w: usize,
    x0: usize,
    y0: usize,
) {
    for (y, row) in layer.chunks_exact(layer_w).enumerate() {
        let dst = &mut canvas[(y0 + y) * canvas_w + x0..][..layer_w];
        for (d, s) in dst.iter_mut().zip(row) {
            *d = over(*s, *d);
        }
    }
}

/// `src` over `dst`, in exact integers scaled by 255².
fn over(src: Pixel, dst: Pixel) -> Pixel {
    match src.a {
        255 => return src,
        0 => return dst,
        _ => {}
    }
    let (sa, da) = (src.a as u32 * 255, dst.a as u32 * (255 - src.a as u32));
    let a = sa + da;
    let c = |s: u8, d: u8| ((s as u32 * sa + d as u32 * da + a / 2) / a) as u8;
    Pixel {
        r: c(src.r, dst.r),
        g: c(src.g, dst.g),
        b: c(src.b, dst.b),
        a: ((a + 127) / 255) as u8,
    }
}

fn floyd_steinberg(pixels: &mut [Pixel], width: usize, height: usize) {
    let mut buf: Vec<[f32; 3]> = pixels
        .iter()
        .map(|p| [p.r as f32, p.g as f32, p.b as f32])
        .collect();

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            let old = buf[idx];
            let q = old.map(quantize_channel);
            (pixels[idx].r, pixels[idx].g, pixels[idx].b) = (q[0], q[1], q[2]);

            let err = [
                old[0] - q[0] as f32,
                old[1] - q[1] as f32,
                old[2] - q[2] as f32,
            ];
            let mut diffuse = |i: usize, factor: f32| {
                for c in 0..3 {
                    buf[i][c] += err[c] * factor;
                }
            };

            if x + 1 < width {
                diffuse(idx + 1, 7.0 / 16.0);
            }
            if y + 1 < height {
                if x > 0 {
                    diffuse(idx + width - 1, 3.0 / 16.0);
                }
                diffuse(idx + width, 5.0 / 16.0);
                if x + 1 < width {
                    diffuse(idx + width + 1, 1.0 / 16.0);
                }
            }
        }
    }
}

/// Quantize a float channel to the nearest 6-level cube value.
fn quantize_channel(v: f32) -> u8 {
    CUBE_LEVELS[nearest_cube_index(v.clamp(0.0, 255.0) as u8)]
}

const BAYER_4X4: [[f32; 4]; 4] = [
    [0.0, 8.0, 2.0, 10.0],
    [12.0, 4.0, 14.0, 6.0],
    [3.0, 11.0, 1.0, 9.0],
    [15.0, 7.0, 13.0, 5.0],
];

fn ordered_dither(pixels: &mut [Pixel], width: usize, height: usize) {
    for y in 0..height {
        for x in 0..width {
            let p = &mut pixels[y * width + x];
            let t = (BAYER_4X4[y % 4][x % 4] / 16.0 - 0.5) * 64.0;
            p.r = quantize_channel(p.r as f32 + t);
            p.g = quantize_channel(p.g as f32 + t);
            p.b = quantize_channel(p.b as f32 + t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(r: u8, g: u8, b: u8, a: u8) -> Pixel {
        Pixel { r, g, b, a }
    }

    /// Build a `cols`×1-cell image from per-cell subpixel patterns: `#` is
    /// `on`, `.` is `off`, read row by row (2 wide, 4 tall).
    fn cells(patterns: &[&str], on: Pixel, off: Pixel) -> Vec<Pixel> {
        let w = patterns.len() * 2;
        let mut v = vec![off; w * 4];
        for (c, pat) in patterns.iter().enumerate() {
            for (i, ch) in pat.chars().enumerate() {
                if ch == '#' {
                    v[(i / 2) * w + c * 2 + i % 2] = on;
                }
            }
        }
        v
    }

    const DARK: Backdrop = Backdrop {
        bg: None,
        light: false,
    };
    const LIGHT: Backdrop = Backdrop {
        bg: None,
        light: true,
    };

    fn block_on(pixels: &[Pixel], cols: usize, backdrop: Backdrop) -> String {
        let mut out = Vec::new();
        let mut p = pixels.to_vec();
        render_block(
            &mut p,
            cols * 2,
            4,
            ColorMode::True,
            Dither::None,
            backdrop,
            &mut out,
        );
        String::from_utf8(out).unwrap()
    }

    fn block(pixels: &[Pixel], cols: usize) -> String {
        block_on(pixels, cols, DARK)
    }

    fn invert(pixels: &[Pixel]) -> Vec<Pixel> {
        pixels
            .iter()
            .map(|p| px(255 - p.r, 255 - p.g, 255 - p.b, p.a))
            .collect()
    }

    const RED: Pixel = Pixel {
        r: 255,
        g: 0,
        b: 0,
        a: 255,
    };
    const BLUE: Pixel = Pixel {
        r: 0,
        g: 0,
        b: 255,
        a: 255,
    };
    const CLEAR: Pixel = Pixel {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    #[test]
    fn dec_table() {
        for v in [0u8, 7, 10, 99, 100, 255] {
            let mut out = Vec::new();
            push_dec(&mut out, v);
            assert_eq!(out, v.to_string().as_bytes());
        }
    }

    #[test]
    fn block_half_combines_fg_and_bg() {
        let s = block(&cells(&["####...."], RED, BLUE), 1);
        assert_eq!(s, "\x1b[38;2;255;0;0;48;2;0;0;255m▀\x1b[0m");
    }

    #[test]
    fn block_uniform_cell_uses_bg_space() {
        let s = block(&cells(&["########"], RED, BLUE), 1);
        assert_eq!(s, "\x1b[48;2;255;0;0m \x1b[0m");
    }

    #[test]
    fn block_fits_every_shape() {
        // Each pattern is one SHAPES mask; the matching glyph must be chosen
        // (or its complement with colors swapped).
        for (pat, g1, g0) in [
            ("#.#.#.#.", '▌', '▐'),
            ("#.#.....", '▘', '▟'),
            (".#.#....", '▝', '▙'),
            ("....#.#.", '▖', '▜'),
            (".....#.#", '▗', '▛'),
            ("#.#..#.#", '▚', '▞'),
        ] {
            let s = block(&cells(&[pat], RED, BLUE), 1);
            assert!(s.contains(g1) || s.contains(g0), "{pat}: {s:?}");
        }
    }

    #[test]
    fn block_picks_glyph_with_fewer_changes() {
        // Second cell is the first flipped: ▄ reuses both colors.
        let s = block(&cells(&["####....", "....####"], RED, BLUE), 2);
        assert_eq!(s, "\x1b[38;2;255;0;0;48;2;0;0;255m▀▄\x1b[0m");
    }

    #[test]
    fn block_transparent() {
        assert_eq!(block(&cells(&["........"], RED, CLEAR), 1), " ");
        let s = block(&cells(&["....####"], RED, CLEAR), 1);
        assert_eq!(s, "\x1b[38;2;255;0;0m▄\x1b[0m");
        let s = block(&cells(&["#.#....."], RED, CLEAR), 1);
        assert_eq!(s, "\x1b[38;2;255;0;0m▘\x1b[0m");
    }

    #[test]
    fn block_translucent_fades_toward_dark() {
        // A translucent edge is antialiased against the assumed-dark terminal.
        let s = block(&cells(&["####...."], px(255, 0, 0, 191), CLEAR), 1);
        assert_eq!(s, "\x1b[38;2;191;0;0m▀\x1b[0m");
    }

    #[test]
    fn block_translucent_fades_toward_light() {
        // With --light, the same edge is antialiased against white instead.
        let s = block_on(&cells(&["####...."], px(255, 0, 0, 191), CLEAR), 1, LIGHT);
        assert_eq!(s, "\x1b[38;2;255;63;63m▀\x1b[0m");
    }

    #[test]
    fn over_blends_straight_alpha() {
        let half_red = px(255, 0, 0, 128);
        // Opaque destination: a plain mix, staying opaque.
        let p = over(half_red, BLUE);
        assert_eq!((p.r, p.g, p.b, p.a), (128, 0, 127, 255));
        // Transparent destination: the source unchanged.
        let p = over(half_red, CLEAR);
        assert_eq!((p.r, p.g, p.b, p.a), (255, 0, 0, 128));
        // Two halves make three quarters, two parts top to one part below.
        let p = over(half_red, px(0, 0, 255, 128));
        assert_eq!((p.r, p.g, p.b, p.a), (170, 0, 85, 192));
    }

    #[test]
    fn overlay_places_layer() {
        // 1x1 red layer at (1, 0) on a 2x2 blue canvas.
        let mut canvas = vec![BLUE; 4];
        overlay(&mut canvas, 2, &[RED], 1, 1, 0);
        let r: Vec<u8> = canvas.iter().map(|p| p.r).collect();
        assert_eq!(r, [0, 255, 0, 0]);
    }

    fn braille_on(pixels: &[Pixel], cols: usize, backdrop: Backdrop) -> String {
        let mut out = Vec::new();
        let mut p = pixels.to_vec();
        render_braille(&mut p, cols * 2, 4, ColorMode::True, 40, backdrop, &mut out);
        String::from_utf8(out).unwrap()
    }

    fn braille(pixels: &[Pixel], cols: usize) -> String {
        braille_on(pixels, cols, DARK)
    }

    #[test]
    fn braille_light_inks_dark_dots() {
        // On a light background dark pixels are the ink, in their own color,
        // and white is left as blank paper.
        let ink = px(40, 40, 90, 255);
        let white = px(255, 255, 255, 255);
        let s = braille_on(&cells(&["########", "........"], ink, white), 2, LIGHT);
        assert_eq!(s, "\x1b[38;2;40;40;90m\u{28ff}\u{2800}\x1b[0m");
        // Transparency is paper too.
        let s = braille_on(&cells(&["#.#.#.#."], ink, CLEAR), 1, LIGHT);
        assert_eq!(s, "\x1b[38;2;40;40;90m\u{2847}\x1b[0m");
    }

    #[test]
    fn light_is_dark_logic_on_the_inverse() {
        // A light-background render picks exactly the glyphs a dark one picks
        // for the inverted image, in the inverted colors.
        let img: Vec<Pixel> = (0..6 * 2 * 4)
            .map(|i| {
                let v = (i * 37 % 256) as u8;
                px(v, v / 2 + 60, 255 - v, 255)
            })
            .collect();
        let glyphs = |s: String| -> String {
            let mut v = Vec::new();
            let mut esc = false;
            for c in s.chars() {
                match c {
                    '\x1b' => esc = true,
                    'm' if esc => esc = false,
                    _ if !esc => v.push(c),
                    _ => {}
                }
            }
            v.into_iter().collect()
        };
        assert_eq!(
            glyphs(braille_on(&img, 6, LIGHT)),
            glyphs(braille(&invert(&img), 6))
        );
        assert_eq!(ascii_on(&img, 6, LIGHT), ascii_on(&invert(&img), 6, DARK));
    }

    #[test]
    fn braille_full_and_empty() {
        let white = px(255, 255, 255, 255);
        let black = px(0, 0, 0, 255);
        let s = braille(&cells(&["########", "........"], white, black), 2);
        assert_eq!(s, "\x1b[38;2;255;255;255m\u{28ff}\u{2800}\x1b[0m");
    }

    #[test]
    fn braille_dark_side_of_edge_stays_unlit() {
        // Navy text on white: navy is above the global threshold, but black
        // represents it better than the cell color, so it stays unlit.
        let s = braille(
            &cells(&[".#.#.#.#"], px(255, 255, 255, 255), px(40, 40, 90, 255)),
            1,
        );
        assert_eq!(s, "\x1b[38;2;255;255;255m\u{28b8}\x1b[0m");
    }

    #[test]
    fn braille_bright_neighbors_stay_lit() {
        // Blue beside white is not "dark": lighting both beats a black hole.
        let s = braille(
            &cells(&[".#.#.#.#"], px(255, 255, 255, 255), px(51, 85, 255, 255)),
            1,
        );
        assert!(s.contains('\u{28ff}'), "{s:?}");
    }

    fn ascii_on(pixels: &[Pixel], cols: usize, backdrop: Backdrop) -> Vec<u8> {
        let mut out = Vec::new();
        let mut p = pixels.to_vec();
        render_ascii(&mut p, cols * 2, 4, ColorMode::Gray, backdrop, &mut out);
        out
    }

    fn ascii(pixels: &[Pixel], cols: usize) -> Vec<u8> {
        ascii_on(pixels, cols, DARK)
    }

    #[test]
    fn ascii_light_is_space_and_dark_is_dense() {
        let white = px(255, 255, 255, 255);
        let black = px(0, 0, 0, 255);
        let out = ascii_on(&cells(&["........", "########"], black, white), 2, LIGHT);
        assert_eq!(out[0], b' ');
        let (_, cov) = GLYPHS.iter().find(|(c, _)| *c == out[1]).unwrap();
        let mean = cov.iter().map(|&c| c as u32).sum::<u32>() / 8;
        assert!(mean > 160, "{} has mean coverage {mean}", out[1] as char);
        // Ink only in the bottom row still reads as an underscore.
        assert_eq!(
            ascii_on(&cells(&["......##"], black, white), 1, LIGHT),
            b"_"
        );
    }

    #[test]
    fn ascii_dark_is_space_and_bright_is_dense() {
        let white = px(255, 255, 255, 255);
        let black = px(0, 0, 0, 255);
        let out = ascii(&cells(&["........", "########"], white, black), 2);
        assert_eq!(out[0], b' ');
        let (_, cov) = GLYPHS.iter().find(|(c, _)| *c == out[1]).unwrap();
        let mean = cov.iter().map(|&c| c as u32).sum::<u32>() / 8;
        assert!(mean > 160, "{} has mean coverage {mean}", out[1] as char);
    }

    #[test]
    fn ascii_matches_shape() {
        let white = px(255, 255, 255, 255);
        let black = px(0, 0, 0, 255);
        // Ink only in the bottom row reads as an underscore.
        assert_eq!(ascii(&cells(&["......##"], white, black), 1), b"_");
    }

    #[test]
    fn prepare_text_collapses_and_maps() {
        let t = prepare_text(
            "  It\u{2019}s\n\n\tan \u{201c}end\u{201d}\u{2014}caf\u{e9}\u{2026} ".as_bytes(),
        );
        assert_eq!(t.unwrap(), b"It's an \"end\"-caf... ");
        assert_eq!(prepare_text(b" \n\x01\t"), None);
    }

    /// `prepare_text`'s output with italic characters shown in brackets.
    fn marked(s: &str) -> String {
        let mut out = String::new();
        let mut on = false;
        for b in prepare_text(s.as_bytes()).unwrap() {
            if (b & ITALIC != 0) != on {
                on = !on;
                out.push(if on { '[' } else { ']' });
            }
            out.push((b & !ITALIC) as char);
        }
        out
    }

    #[test]
    fn prepare_text_italics() {
        // Gutenberg markup: words and runs, beside punctuation, across lines.
        assert_eq!(marked("so _very_ remarkable"), "so [very] remarkable ");
        assert_eq!(marked("\"_Alice!_\" she"), "\"[Alice!]\" she ");
        assert_eq!(marked("_two\nlines_ end"), "[two lines] end ");
        // Literal underscores stay.
        assert_eq!(marked("snake_case a _ b x_"), "snake_case a _ b x_ ");
        // A stray opener ends at the paragraph.
        assert_eq!(marked("_stray\n\nnext"), "[stray] next ");
    }

    fn text_on(
        pixels: &[Pixel],
        cols: usize,
        color: ColorMode,
        backdrop: Backdrop,
        t: &[u8],
    ) -> String {
        let mut out = Vec::new();
        let mut p = pixels.to_vec();
        render_text(&mut p, cols * 2, 4, color, backdrop, t, &mut out);
        String::from_utf8(out).unwrap()
    }

    fn text(pixels: &[Pixel], cols: usize, color: ColorMode, t: &[u8]) -> String {
        text_on(pixels, cols, color, DARK, t)
    }

    #[test]
    fn text_light_compensates_with_darker_ink() {
        // On paper a thin glyph needs darker ink than a heavy one to read as
        // the same gray, and a black cell is capped at black.
        let gray = cells(&["########"], px(100, 100, 100, 255), CLEAR);
        let thin = text_on(&gray, 1, ColorMode::True, LIGHT, b"i");
        let heavy = text_on(&gray, 1, ColorMode::True, LIGHT, b"@");
        let level = |s: &str| s[7..].split(';').next().unwrap().parse::<u32>().unwrap();
        assert!(
            level(&thin) < 100 && level(&heavy) > 100,
            "{thin:?} {heavy:?}"
        );

        let black = cells(&["########"], px(0, 0, 0, 255), CLEAR);
        assert_eq!(
            text_on(&black, 1, ColorMode::True, LIGHT, b"."),
            "\x1b[38;2;0;0;0m.\x1b[0m"
        );
    }

    #[test]
    fn text_advances_only_in_lit_cells_and_repeats() {
        let white = px(255, 255, 255, 255);
        let black = px(0, 0, 0, 255);
        let lit = "########";
        let dark = "........";
        let img = cells(&[lit, dark, lit, lit, dark, lit], white, black);
        // Lit cells take a, b, the joining space, then the text repeats.
        assert_eq!(text(&img, 6, ColorMode::Gray, b"ab "), "a b  a");
    }

    #[test]
    fn text_compensates_coverage() {
        // On the same gray, a thin glyph is drawn brighter than a heavy one,
        // and a bright cell is capped at full white rather than wrapping.
        let gray = cells(&["########"], px(100, 100, 100, 255), px(0, 0, 0, 255));
        let thin = text(&gray, 1, ColorMode::True, b"i");
        let heavy = text(&gray, 1, ColorMode::True, b"@");
        let level = |s: &str| s[7..].split(';').next().unwrap().parse::<u32>().unwrap();
        assert!(
            level(&thin) > 100 && level(&heavy) < 100,
            "{thin:?} {heavy:?}"
        );

        let white = cells(&["########"], px(255, 255, 255, 255), px(0, 0, 0, 255));
        assert_eq!(
            text(&white, 1, ColorMode::True, b"."),
            "\x1b[38;2;255;255;255m.\x1b[0m"
        );
    }
}
