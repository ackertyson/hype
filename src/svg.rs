//! SVG output (cargo feature `svg`): ascii, `--text` and braille drawn as
//! vector shapes, with glyph outlines and coverage from the Recursive font
//! (`recursive.rs`). The glyph and dot decisions are the terminal modes'
//! own; only the drawing differs.
//!
//! Each cell is 1×2 user units. Every shape used is defined once in
//! `<defs>`, scaled to the cell, and each drawn cell is a `<use>` of it in
//! its color, so output grows by one short element per non-blank cell.
//!
//! Glyphs come in several weights (`recursive::WEIGHTS`), merged into one
//! ramp: the density ramp and shape matching pick the weight along with the
//! glyph, so dark (on paper) areas get heavy strokes and light ones regular.
//! `--text` italics (`render::ITALIC`) use Recursive's true italics.

use crate::Fit;
use crate::color::{ansi256_rgb, nearest_ansi256};
use crate::decode::Image;
use crate::recursive::{
    FILL_TRANSFORM, ITALIC_FILL, ITALIC_NATURAL, ITALIC_PATHS, NATURAL_TRANSFORM, OTHER_FILL,
    OTHER_NATURAL, PATHS, RAMP_FILL, RAMP_NATURAL, WEIGHTS,
};
use crate::render::{self, Backdrop, ColorMode, Rgb};
use std::io::Write;

/// A glyph, labeled (character, index into `WEIGHTS`), and its coverage.
type Glyph = ((u8, u8), [u8; 8]);

/// The coverage tables for one fit.
struct Tables {
    /// The density ramp of all weights, as `render::GLYPHS` is for the
    /// terminal.
    ramp: &'static [Glyph],
    /// Every other printable glyph at each weight, for `--text`.
    other: &'static [Glyph],
    /// Every printable italic glyph at each weight, for `--text`.
    italic: &'static [Glyph],
    /// `render::ref_ink` for each weight, upright and italic: text in a
    /// cell is shown as bright as an average lowercase letter of the weight
    /// and style drawn there.
    ref_ink: [u32; WEIGHTS.len()],
    italic_ref_ink: [u32; WEIGHTS.len()],
    /// Maps glyph outlines onto the 1×2 cell. Precomputed as text:
    /// formatting floats here added 13 KB of code.
    transform: &'static str,
}

/// Total ink of an average lowercase letter at each weight in `ramp`.
const fn ref_inks(ramp: &[Glyph]) -> [u32; WEIGHTS.len()] {
    let mut sums = [0; WEIGHTS.len()];
    let mut counts = [0; WEIGHTS.len()];
    let mut i = 0;
    while i < ramp.len() {
        let ((ch, w), cov) = ramp[i];
        if ch.is_ascii_lowercase() {
            let mut j = 0;
            while j < 8 {
                sums[w as usize] += cov[j] as u32;
                j += 1;
            }
            counts[w as usize] += 1;
        }
        i += 1;
    }
    let mut w = 0;
    while w < WEIGHTS.len() {
        sums[w] /= counts[w];
        w += 1;
    }
    sums
}

const NATURAL: Tables = Tables {
    ramp: &RAMP_NATURAL,
    other: &OTHER_NATURAL,
    italic: &ITALIC_NATURAL,
    ref_ink: ref_inks(&RAMP_NATURAL),
    italic_ref_ink: ref_inks(&ITALIC_NATURAL),
    transform: NATURAL_TRANSFORM,
};

const FILLED: Tables = Tables {
    ramp: &RAMP_FILL,
    other: &OTHER_FILL,
    italic: &ITALIC_FILL,
    ref_ink: ref_inks(&RAMP_FILL),
    italic_ref_ink: ref_inks(&ITALIC_FILL),
    transform: FILL_TRANSFORM,
};

/// Braille dot radius, in cells (a subpixel is 0.5 across).
const DOT_R: &str = "0.22";
/// Subpixel centers, in cells: x by column, y by row.
const DOT_X: [&str; 2] = ["0.25", "0.75"];
const DOT_Y: [&str; 4] = ["0.25", "0.75", "1.25", "1.75"];

/// A drawn cell: column, row, shape, and color (None: the single ink color).
type Mark = (usize, usize, Shape, Option<Rgb>);

/// A glyph by its (character, weight index), upright or italic, or a
/// braille cell by its subpixel mask.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Shape {
    Glyph((u8, u8)),
    Italic((u8, u8)),
    Dots(u8),
}

/// A shape's element id: `g<weight index>-<char code>` for a glyph,
/// `i<weight index>-<char code>` for an italic one, `b<mask>` for braille
/// dots.
struct Id(Shape);

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.0 {
            Shape::Glyph((ch, w)) => write!(f, "g{w}-{ch}"),
            Shape::Italic((ch, w)) => write!(f, "i{w}-{ch}"),
            Shape::Dots(mask) => write!(f, "b{mask}"),
        }
    }
}

/// The color to draw `c` (from a `flatten`ed image) in: inverted back on a
/// light background, and as the terminal would show it for `-c 256`. Gray
/// draws everything in one ink color, as the terminal modes then emit no
/// color at all.
fn ink(c: Rgb, color: ColorMode, backdrop: Backdrop) -> Option<Rgb> {
    let c = if backdrop.light {
        (255 - c.0, 255 - c.1, 255 - c.2)
    } else {
        c
    };
    match color {
        ColorMode::True => Some(c),
        ColorMode::Ansi256 => Some(ansi256_rgb(nearest_ansi256(c.0, c.1, c.2))),
        ColorMode::Gray => None,
    }
}

/// ASCII mode, or `--text` if given, as SVG.
pub fn ascii(
    img: &mut Image,
    color: ColorMode,
    backdrop: Backdrop,
    fit: Fit,
    text: Option<&[u8]>,
    out: &mut Vec<u8>,
) {
    let t = match fit {
        Fit::Fill => &FILLED,
        Fit::Natural => &NATURAL,
    };
    render::flatten(&mut img.pixels, backdrop);
    let mut text = text.map(|s| s.iter().copied().cycle());
    let (cols, rows) = (img.width / 2, img.height / 4);
    let mut marks = Vec::new();
    for row in 0..rows {
        for col in 0..cols {
            let sub = render::cell(&img.pixels, img.width, col, row);
            let &(g, cov) = render::ascii_glyph(&sub, t.ramp);
            if g.0 == b' ' {
                continue;
            }
            let (shape, c) = match &mut text {
                None => (Shape::Glyph(g), render::ink_mean(&sub, &cov)),
                Some(text) => {
                    let ch = text.next().unwrap_or(b' ');
                    let italic = ch & render::ITALIC != 0;
                    let ch = ch & !render::ITALIC;
                    if ch == b' ' {
                        continue;
                    }
                    let (refs, table) = if italic {
                        (&t.italic_ref_ink, t.italic)
                    } else {
                        (&t.ref_ink, t.other)
                    };
                    // The weight whose average letter has the ink closest to
                    // what ascii mode puts here, so tone follows ascii mode
                    // (not its glyph's weight: a heavy '.' is still faint).
                    let ink: u32 = cov.iter().map(|&v| v as u32).sum();
                    let w = (0..WEIGHTS.len())
                        .min_by_key(|&w| refs[w].abs_diff(ink))
                        .unwrap();
                    // Every printable glyph is measured at each weight and
                    // style; all have some ink.
                    let g = (ch, w as u8);
                    let upright = if italic { &[][..] } else { t.ramp };
                    let (_, cov) = upright.iter().chain(table).find(|x| x.0 == g).unwrap();
                    let c = render::compensate(render::ink_mean(&sub, cov), cov, refs[w]);
                    let shape = if italic {
                        Shape::Italic(g)
                    } else {
                        Shape::Glyph(g)
                    };
                    (shape, c)
                }
            };
            marks.push((col, row, shape, ink(c, color, backdrop)));
        }
    }
    write(out, cols, rows, &marks, backdrop, t);
}

/// Braille mode as SVG: lit dots as circles.
pub fn braille(
    img: &mut Image,
    color: ColorMode,
    threshold: u8,
    backdrop: Backdrop,
    out: &mut Vec<u8>,
) {
    render::flatten(&mut img.pixels, backdrop);
    let limit = threshold as u32 * 1000;
    let (cols, rows) = (img.width / 2, img.height / 4);
    let mut marks = Vec::new();
    for row in 0..rows {
        for col in 0..cols {
            let sub = render::cell(&img.pixels, img.width, col, row);
            let mask = render::lit_mask(&sub, limit);
            if mask != 0 {
                let c = ink(render::mean(&sub, mask), color, backdrop);
                marks.push((col, row, Shape::Dots(mask), c));
            }
        }
    }
    write(out, cols, rows, &marks, backdrop, &NATURAL);
}

/// The SVG document: background, the shapes used, then the marks.
fn write(
    out: &mut Vec<u8>,
    cols: usize,
    rows: usize,
    marks: &[Mark],
    backdrop: Backdrop,
    t: &Tables,
) {
    let (paper, ink) = if backdrop.light {
        ("#fff", "#000")
    } else {
        ("#000", "#fff")
    };
    let (w, h) = (cols, rows * 2);
    // Sized like a terminal at about 10×20-pixel cells; viewers and print
    // dialogs scale it freely.
    let _ = write!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w} {h}\" width=\"{}\" height=\"{}\">\n\
         <rect width=\"{w}\" height=\"{h}\" fill=\"{paper}\"/>\n<defs>\n",
        w * 10,
        h * 10,
    );
    let mut used: Vec<Shape> = marks.iter().map(|m| m.2).collect();
    used.sort_unstable();
    used.dedup();
    for shape in used {
        match shape {
            Shape::Glyph(g) | Shape::Italic(g) => {
                let paths = match shape {
                    Shape::Italic(_) => &ITALIC_PATHS,
                    _ => &PATHS,
                };
                let d = paths.iter().find(|p| p.0 == g).unwrap().1;
                let id = Id(shape);
                let _ = writeln!(
                    out,
                    "<path id=\"{id}\" transform=\"{}\" d=\"{d}\"/>",
                    t.transform
                );
            }
            Shape::Dots(mask) => {
                let _ = write!(out, "<g id=\"{}\">", Id(shape));
                for i in (0..8).filter(|i| mask >> i & 1 == 1) {
                    let (cx, cy) = (DOT_X[i % 2], DOT_Y[i / 2]);
                    let _ = write!(out, "<circle cx=\"{cx}\" cy=\"{cy}\" r=\"{DOT_R}\"/>");
                }
                out.extend_from_slice(b"</g>\n");
            }
        }
    }
    let _ = writeln!(out, "</defs>\n<g fill=\"{ink}\">");
    for &(col, row, shape, c) in marks {
        let (id, y) = (Id(shape), row * 2);
        let _ = write!(out, "<use href=\"#{id}\" x=\"{col}\" y=\"{y}\"");
        if let Some((r, g, b)) = c {
            let _ = write!(out, " fill=\"#{r:02x}{g:02x}{b:02x}\"");
        }
        out.extend_from_slice(b"/>\n");
    }
    out.extend_from_slice(b"</g>\n</svg>");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Pixel;

    const DARK: Backdrop = Backdrop {
        bg: None,
        light: false,
    };
    const LIGHT: Backdrop = Backdrop {
        bg: None,
        light: true,
    };

    /// A `cols`×1-cell image of solid cells.
    fn solid(colors: &[Rgb]) -> Image {
        let width = colors.len() * 2;
        let pixels = (0..width * 4)
            .map(|i| {
                let (r, g, b) = colors[i % width / 2];
                Pixel { r, g, b, a: 255 }
            })
            .collect();
        Image {
            pixels,
            width,
            height: 4,
        }
    }

    fn svg_ascii(img: &[Rgb], color: ColorMode, bd: Backdrop, text: Option<&[u8]>) -> String {
        let mut out = Vec::new();
        ascii(&mut solid(img), color, bd, Fit::Fill, text, &mut out);
        String::from_utf8(out).unwrap()
    }

    fn uses(s: &str) -> Vec<&str> {
        s.lines().filter(|l| l.starts_with("<use")).collect()
    }

    #[test]
    fn ascii_draws_lit_cells_in_their_color() {
        let s = svg_ascii(&[(255, 255, 255), (0, 0, 0)], ColorMode::True, DARK, None);
        assert!(s.starts_with("<svg ") && s.ends_with("</svg>"), "{s}");
        assert!(s.contains("viewBox=\"0 0 2 2\""), "{s}");
        assert!(s.contains("<rect width=\"2\" height=\"2\" fill=\"#000\"/>"));
        // The bright cell gets the densest glyph; the black cell is blank.
        let u = uses(&s);
        assert_eq!(u.len(), 1, "{s}");
        let dense = Id(Shape::Glyph(RAMP_FILL.last().unwrap().0));
        assert_eq!(
            u[0],
            format!("<use href=\"#{dense}\" x=\"0\" y=\"0\" fill=\"#ffffff\"/>")
        );
        // Exactly the glyphs used are defined.
        assert_eq!(s.matches("<path id=").count(), 1);
        assert!(s.contains(&format!("<path id=\"{dense}\"")));
    }

    /// The weight index of each drawn glyph, in order.
    fn weights(s: &str) -> Vec<u32> {
        uses(s)
            .iter()
            .map(|u| u["<use href=\"#g".len()..].chars().next().unwrap())
            .map(|c| c.to_digit(10).unwrap())
            .collect()
    }

    /// A fine gray ramp, black to white.
    fn gray_ramp() -> Vec<Rgb> {
        (0..64u32)
            .map(|i| {
                let v = (i * 255 / 63) as u8;
                (v, v, v)
            })
            .collect()
    }

    #[test]
    fn text_weight_steps_smoothly_with_tone() {
        // Weight follows tone alone: across a gradient it never decreases
        // and never skips a weight, from regular to the heaviest.
        let s = svg_ascii(&gray_ramp(), ColorMode::True, DARK, Some(b"ab "));
        let w = weights(&s);
        assert!(
            w.windows(2).all(|p| p[1] == p[0] || p[1] == p[0] + 1),
            "{w:?}"
        );
        assert_eq!(
            (w[0], w[w.len() - 1]),
            (0, WEIGHTS.len() as u32 - 1),
            "{w:?}"
        );
    }

    #[test]
    fn ascii_weight_rises_with_tone() {
        // Glyphs are picked by coverage at any weight (a heavy '.' is still
        // faint), so weight rises with tone on average, to the heaviest.
        let w = weights(&svg_ascii(&gray_ramp(), ColorMode::True, DARK, None));
        let (dim, bright) = w.split_at(w.len() / 2);
        let avg = |v: &[u32]| v.iter().sum::<u32>() as f64 / v.len() as f64;
        assert!(avg(dim) < avg(bright), "{w:?}");
        assert_eq!(w[w.len() - 1], WEIGHTS.len() as u32 - 1, "{w:?}");
    }

    #[test]
    fn ascii_light_inks_dark_cells() {
        let s = svg_ascii(
            &[(255, 255, 255), (20, 40, 60)],
            ColorMode::True,
            LIGHT,
            None,
        );
        assert!(s.contains("fill=\"#fff\"/>"), "paper: {s}");
        let u = uses(&s);
        assert_eq!(u.len(), 1, "{s}");
        assert!(
            u[0].contains("x=\"1\"") && u[0].ends_with("fill=\"#14283c\"/>"),
            "{s}"
        );
    }

    #[test]
    fn gray_uses_one_ink_color() {
        let s = svg_ascii(&[(200, 100, 50)], ColorMode::Gray, LIGHT, None);
        assert!(s.contains("<g fill=\"#000\">"), "{s}");
        assert!(!uses(&s)[0].contains("fill"), "{s}");
    }

    #[test]
    fn ansi256_uses_palette_colors() {
        let s = svg_ascii(&[(250, 5, 5)], ColorMode::Ansi256, DARK, None);
        assert!(uses(&s)[0].ends_with("fill=\"#ff0000\"/>"), "{s}");
    }

    #[test]
    fn text_advances_only_in_lit_cells() {
        let w = (255, 255, 255);
        let k = (0, 0, 0);
        let s = svg_ascii(&[w, k, w, w], ColorMode::True, DARK, Some(b"ab "));
        let u = uses(&s);
        // a, b, then the joining space leaves the third lit cell blank. White
        // cells are the brightest tone, so the text takes the heaviest weight.
        let top = WEIGHTS.len() - 1;
        assert_eq!(u.len(), 2, "{s}");
        assert!(
            u[0].starts_with(&format!("<use href=\"#g{top}-97\" x=\"0\"")),
            "{s}"
        );
        assert!(
            u[1].starts_with(&format!("<use href=\"#g{top}-98\" x=\"2\"")),
            "{s}"
        );
        // A faint cell takes the regular weight.
        let s = svg_ascii(&[(40, 40, 40)], ColorMode::True, DARK, Some(b"a "));
        assert!(uses(&s)[0].starts_with("<use href=\"#g0-97\""), "{s}");
        // Coverage is compensated within the weight: a thin glyph on white
        // is brightened, capped at white.
        let s = svg_ascii(&[w], ColorMode::True, DARK, Some(b". "));
        assert!(uses(&s)[0].ends_with("fill=\"#ffffff\"/>"), "{s}");
    }

    #[test]
    fn text_italics_use_italic_outlines() {
        // "a _b_" as prepare_text marks it: the b is italic.
        let text = render::prepare_text(b"a _b_").unwrap();
        let w = (255, 255, 255);
        let s = svg_ascii(&[w, w, w], ColorMode::True, DARK, Some(&text));
        let u = uses(&s);
        let top = WEIGHTS.len() - 1;
        assert!(
            u[0].starts_with(&format!("<use href=\"#g{top}-97\"")),
            "{s}"
        );
        assert!(
            u[1].starts_with(&format!("<use href=\"#i{top}-98\"")),
            "{s}"
        );
        // Defined from the italic outlines.
        let d = ITALIC_PATHS
            .iter()
            .find(|p| p.0 == (b'b', top as u8))
            .unwrap()
            .1;
        assert!(
            s.contains(&format!(
                "<path id=\"i{top}-98\" transform=\"{FILL_TRANSFORM}\" d=\"{d}\"/>"
            )),
            "{s}"
        );
        // And weight still follows tone for italics: a faint cell is regular.
        let s = svg_ascii(
            &[(40, 40, 40)],
            ColorMode::True,
            DARK,
            Some(&[b'b' | render::ITALIC, b' ']),
        );
        assert!(uses(&s)[0].starts_with("<use href=\"#i0-98\""), "{s}");
    }

    #[test]
    fn fits_map_their_box_onto_the_cell() {
        // Each fit's box corners (font units, cell-local) land on the 1×2
        // cell's corners: natural is the whole 600×1200 cell, fill the box
        // from (30, 190) to (570, 1150). Path data is in half font units.
        for (tr, (x0, y0, x1, y1)) in [
            (NATURAL_TRANSFORM, (0.0, 0.0, 600.0, 1200.0)),
            (FILL_TRANSFORM, (30.0, 190.0, 570.0, 1150.0)),
        ] {
            let m: Vec<f64> = tr
                .strip_prefix("matrix(")
                .and_then(|s| s.strip_suffix(')'))
                .unwrap()
                .split(' ')
                .map(|v| v.parse().unwrap())
                .collect();
            let at = |x: f64, y: f64| (m[0] * 2.0 * x + m[4], m[3] * 2.0 * y + m[5]);
            let near = |(a, b): (f64, f64), (c, d): (f64, f64)| {
                (a - c).abs() < 1e-9 && (b - d).abs() < 1e-9 && m[1] == 0.0 && m[2] == 0.0
            };
            assert!(near(at(x0, y0), (0.0, 0.0)), "{tr}");
            assert!(near(at(x1, y1), (1.0, 2.0)), "{tr}");
        }
        // And the fit chooses which one the glyphs use.
        let mut out = Vec::new();
        let white = &[(255, 255, 255)];
        ascii(
            &mut solid(white),
            ColorMode::True,
            DARK,
            Fit::Natural,
            None,
            &mut out,
        );
        let s = String::from_utf8(out).unwrap();
        assert!(
            s.contains(&format!("transform=\"{NATURAL_TRANSFORM}\"")),
            "{s}"
        );
    }

    #[test]
    fn braille_draws_lit_dots_as_circles() {
        let mut out = Vec::new();
        let mut img = solid(&[(255, 255, 255), (0, 0, 0)]);
        braille(&mut img, ColorMode::True, 40, DARK, &mut out);
        let s = String::from_utf8(out).unwrap();
        assert_eq!(s.matches("<circle ").count(), 8, "{s}");
        assert!(
            s.contains("<circle cx=\"0.75\" cy=\"1.75\" r=\"0.22\"/>"),
            "{s}"
        );
        assert_eq!(
            uses(&s),
            ["<use href=\"#b255\" x=\"0\" y=\"0\" fill=\"#ffffff\"/>"]
        );
    }
}
