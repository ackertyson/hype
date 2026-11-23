#[cfg(not(any(
    feature = "png",
    feature = "jpeg",
    feature = "gif",
    feature = "webp",
    feature = "bmp"
)))]
compile_error!("enable at least one image format feature (png, jpeg, gif, webp, bmp)");

mod color;
mod decode;
#[cfg(feature = "svg")]
mod recursive;
mod render;
mod resize;
#[cfg(feature = "svg")]
mod svg;
mod term;

use render::{ColorMode, Dither};
use std::io::Write;
use std::process;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const LOGO: &[u8] = include_bytes!("../banana.png");

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = match parse_args(&args, detect_color_support()) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("hype: {e}");
            process::exit(1);
        }
    };

    if opts.help {
        print_help(opts.color, opts.light);
        return;
    }
    if opts.version {
        emit(format!("hype {VERSION}\n").as_bytes());
        return;
    }

    let Some((base, layers)) = opts.images.split_first() else {
        eprintln!("hype: no image specified (use -h for help)");
        process::exit(1);
    };

    let text = opts.text.as_ref().map(|path| {
        let fail = |e: String| -> ! {
            eprintln!("hype: {e}");
            process::exit(1);
        };
        let (bytes, name) = if path == "-" {
            // Waiting on keyboard input would look like a hang.
            if term::is_tty(0) {
                fail(
                    "--text reads stdin, but stdin is a terminal (pipe text in, or give a file)"
                        .into(),
                );
            }
            let mut bytes = Vec::new();
            if let Err(e) = std::io::Read::read_to_end(&mut std::io::stdin(), &mut bytes) {
                fail(format!("cannot read stdin: {e}"));
            }
            (bytes, "stdin".to_string())
        } else {
            let bytes =
                std::fs::read(path).unwrap_or_else(|e| fail(format!("cannot read '{path}': {e}")));
            (bytes, format!("'{path}'"))
        };
        render::prepare_text(&bytes)
            .unwrap_or_else(|| fail(format!("{name} has no printable text")))
    });

    let term_width = term::width().unwrap_or(80);
    let mut img = open(base, |img_w, img_h| {
        target_size(opts.width, opts.height, term_width, img_w, img_h)
    });
    let (w, h) = (img.width, img.height);
    // Each further layer is fit inside the canvas, centered, and composited
    // over it in order.
    for path in layers {
        let layer = open(path, |img_w, img_h| fit_size(w, h, img_w, img_h));
        let (x0, y0) = ((w - layer.width) / 2, (h - layer.height) / 2);
        render::overlay(&mut img.pixels, w, &layer.pixels, layer.width, x0, y0);
    }

    let mut out = Vec::with_capacity(w * h * 12);
    let bd = render::Backdrop {
        bg: opts.bg,
        light: opts.light,
    };
    if let Some(fit) = opts.svg {
        svg_out(&mut img, &opts, fit, bd, text.as_deref(), &mut out);
    } else {
        render_text_art(&mut img, &opts, bd, text.as_deref(), &mut out);
    }
    out.push(b'\n');

    emit(&out);
}

fn render_text_art(
    img: &mut decode::Image,
    opts: &Opts,
    bd: render::Backdrop,
    text: Option<&[u8]>,
    out: &mut Vec<u8>,
) {
    let (w, h, px) = (img.width, img.height, &mut img.pixels);
    match opts.mode {
        Mode::Block => render::render_block(px, w, h, opts.color, opts.dither, bd, out),
        Mode::Braille => render::render_braille(px, w, h, opts.color, opts.threshold, bd, out),
        Mode::Ascii => match text {
            Some(t) => render::render_text(px, w, h, opts.color, bd, t, out),
            None => render::render_ascii(px, w, h, opts.color, bd, out),
        },
    }
}

#[cfg(feature = "svg")]
fn svg_out(
    img: &mut decode::Image,
    opts: &Opts,
    fit: Fit,
    bd: render::Backdrop,
    text: Option<&[u8]>,
    out: &mut Vec<u8>,
) {
    match opts.mode {
        Mode::Braille => svg::braille(img, opts.color, opts.threshold, bd, opts.tint, out),
        _ => svg::ascii(img, opts.color, bd, fit, text, opts.tint, out),
    }
}

#[cfg(not(feature = "svg"))]
fn svg_out(
    _: &mut decode::Image,
    _: &Opts,
    _: Fit,
    _: render::Backdrop,
    _: Option<&[u8]>,
    _: &mut Vec<u8>,
) {
    unreachable!("parse_args rejects --svg without the svg feature");
}

/// How `--svg` fits glyphs to their cells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fit {
    /// Enlarged to fill the cell: more ink, so a wider tonal range.
    Fill,
    /// The font's own proportions.
    Natural,
}

fn open(path: &str, size: impl Fn(usize, usize) -> (usize, usize)) -> decode::Image {
    decode::load(path, size).unwrap_or_else(|e| {
        eprintln!("hype: cannot open '{path}': {e}");
        process::exit(1);
    })
}

/// Resampled pixel dimensions: every mode samples each character cell as
/// 2×4 subpixels. A side not given follows the image's aspect ratio (cells
/// are about twice as tall as wide); with neither, the width is
/// `default_cols`. Both given: stretch.
fn target_size(
    width: Option<usize>,
    height: Option<usize>,
    default_cols: usize,
    img_w: usize,
    img_h: usize,
) -> (usize, usize) {
    let (cols, rows) = match (width, height) {
        (Some(cols), Some(rows)) => (cols, rows),
        (None, Some(rows)) => ((rows * 2 * img_w + img_h / 2) / img_h, rows),
        (cols, None) => {
            let cols = cols.unwrap_or(default_cols).max(1);
            let rows = ((cols as f64 * img_h as f64 / img_w as f64) as usize).div_ceil(2);
            (cols, rows)
        }
    };
    (cols.max(1) * 2, rows.max(1) * 4)
}

/// Largest size with the image's aspect ratio that fits a `canvas_w`×`canvas_h`
/// canvas. Subpixels are about square, so no cell-shape correction is needed.
fn fit_size(canvas_w: usize, canvas_h: usize, img_w: usize, img_h: usize) -> (usize, usize) {
    if img_w * canvas_h <= img_h * canvas_w {
        let w = (img_w * canvas_h + img_h / 2) / img_h;
        (w.clamp(1, canvas_w), canvas_h)
    } else {
        let h = (img_h * canvas_w + img_w / 2) / img_w;
        (canvas_w, h.clamp(1, canvas_h))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Block,
    Braille,
    Ascii,
}

struct Opts {
    images: Vec<String>,
    layer: bool,
    width: Option<usize>,
    height: Option<usize>,
    mode: Mode,
    color: ColorMode,
    dither: Dither,
    threshold: u8,
    bg: Option<(u8, u8, u8)>,
    light: bool,
    text: Option<String>,
    svg: Option<Fit>,
    /// `--tint` strength, percent of the ink (0: off).
    tint: u32,
    help: bool,
    version: bool,
}

/// Detect terminal color support from environment variables.
fn detect_color_support() -> ColorMode {
    // COLORTERM=truecolor or 24bit indicates 24-bit color support
    if let Ok(ct) = std::env::var("COLORTERM")
        && (ct == "truecolor" || ct == "24bit")
    {
        return ColorMode::True;
    }
    // Fall back to 256-color (safe for xterm-256color and most modern terminals)
    ColorMode::Ansi256
}

fn parse_args(args: &[String], default_color: ColorMode) -> Result<Opts, String> {
    let mut opts = Opts {
        images: Vec::new(),
        layer: false,
        width: None,
        height: None,
        mode: Mode::Block,
        color: default_color,
        dither: Dither::None,
        threshold: 40,
        bg: None,
        light: false,
        text: None,
        svg: None,
        tint: 0,
        help: false,
        version: false,
    };
    let mut mode = None;
    let mut color = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" | "help" => opts.help = true,
            "-v" | "--version" => opts.version = true,
            "-l" | "--layer" => opts.layer = true,
            "-w" | "--width" => {
                i += 1;
                opts.width = Some(parse_num(args, i, "width")?);
            }
            "-H" | "--height" => {
                i += 1;
                opts.height = Some(parse_num(args, i, "height")?);
            }
            "-m" | "--mode" => {
                i += 1;
                let val = next_arg(args, i, "mode")?;
                mode = Some(match val.as_str() {
                    "block" => Mode::Block,
                    "braille" => Mode::Braille,
                    "ascii" => Mode::Ascii,
                    _ => return Err(format!("unknown mode '{val}' (block, braille, ascii)")),
                });
            }
            "--text" => {
                // Without a file (last, or followed by an option): stdin.
                opts.text = Some(match args.get(i + 1) {
                    Some(f) if f == "-" || !f.starts_with('-') => {
                        i += 1;
                        f.clone()
                    }
                    _ => "-".into(),
                });
            }
            "-c" | "--color" => {
                i += 1;
                let val = next_arg(args, i, "color")?;
                color = Some(match val.as_str() {
                    "true" => ColorMode::True,
                    "256" => ColorMode::Ansi256,
                    "gray" | "grey" => ColorMode::Gray,
                    _ => return Err(format!("unknown color mode '{val}' (true, 256, gray)")),
                });
            }
            "--svg" => {
                // The fit is optional: only these words are taken as one.
                opts.svg = Some(match args.get(i + 1).map(String::as_str) {
                    Some("fill") => {
                        i += 1;
                        Fit::Fill
                    }
                    Some("natural") => {
                        i += 1;
                        Fit::Natural
                    }
                    _ => Fit::Fill,
                });
            }
            "--tint" => {
                // The strength is optional: taken only if it is a number.
                opts.tint = match args.get(i + 1).and_then(|v| v.parse::<u32>().ok()) {
                    Some(n) if n <= 100 => {
                        i += 1;
                        n
                    }
                    Some(_) => return Err("tint must be 0-100".into()),
                    None => 50,
                };
            }
            "-d" | "--dither" => {
                i += 1;
                let val = next_arg(args, i, "dither")?;
                opts.dither = match val.as_str() {
                    "fs" => Dither::FloydSteinberg,
                    "ordered" => Dither::Ordered,
                    "none" => Dither::None,
                    _ => return Err(format!("unknown dither '{val}' (fs, ordered, none)")),
                };
            }
            "-t" | "--threshold" => {
                i += 1;
                let n = parse_num::<u16>(args, i, "threshold")?;
                if n > 255 {
                    return Err("threshold must be 0-255".into());
                }
                opts.threshold = n as u8;
            }
            "-b" | "--bg" => {
                i += 1;
                let val = next_arg(args, i, "bg")?;
                opts.bg = Some(parse_bg(&val)?);
            }
            "--light" => opts.light = true,
            other => {
                if other.starts_with('-') {
                    return Err(format!("unknown option '{other}'"));
                }
                opts.images.push(other.to_string());
            }
        }
        i += 1;
    }
    opts.mode = match (mode, &opts.text) {
        (None | Some(Mode::Ascii), Some(_)) => Mode::Ascii,
        (Some(_), Some(_)) => return Err("--text works in ascii mode only".into()),
        // SVG draws ascii or braille, so it defaults to ascii.
        (None, None) if opts.svg.is_some() => Mode::Ascii,
        (m, None) => m.unwrap_or(Mode::Block),
    };
    if opts.svg.is_some() {
        if !cfg!(feature = "svg") {
            return Err("--svg is not in this build (cargo feature svg)".into());
        }
        if opts.mode == Mode::Block {
            return Err("--svg draws ascii or braille mode, not block".into());
        }
        // A file has no terminal to detect: full color unless asked.
        opts.color = color.unwrap_or(ColorMode::True);
    } else if opts.tint > 0 {
        return Err("--tint works with --svg only".into());
    } else if let Some(c) = color {
        opts.color = c;
    }
    // Several images are only layered when asked for, so a stray argument or
    // an unexpected glob match stays an error.
    if let [_, extra, ..] = &opts.images[..]
        && !opts.layer
    {
        return Err(format!(
            "unexpected argument '{extra}' (use --layer to overlay images)"
        ));
    }
    Ok(opts)
}

fn next_arg(args: &[String], i: usize, name: &str) -> Result<String, String> {
    args.get(i)
        .cloned()
        .ok_or_else(|| format!("missing value for --{name}"))
}

fn parse_num<T: std::str::FromStr>(args: &[String], i: usize, name: &str) -> Result<T, String> {
    let val = next_arg(args, i, name)?;
    val.parse::<T>()
        .map_err(|_| format!("invalid number for --{name}: '{val}'"))
}

fn parse_bg(s: &str) -> Result<(u8, u8, u8), String> {
    match s {
        "black" => Ok((0, 0, 0)),
        "white" => Ok((255, 255, 255)),
        _ => {
            let parts: Vec<&str> = s.split(',').collect();
            if parts.len() != 3 {
                return Err(format!("invalid bg '{s}' (use black, white, or R,G,B)"));
            }
            let r = parts[0]
                .parse::<u8>()
                .map_err(|_| format!("invalid bg red: '{}'", parts[0]))?;
            let g = parts[1]
                .parse::<u8>()
                .map_err(|_| format!("invalid bg green: '{}'", parts[1]))?;
            let b = parts[2]
                .parse::<u8>()
                .map_err(|_| format!("invalid bg blue: '{}'", parts[2]))?;
            Ok((r, g, b))
        }
    }
}

/// Visible width of a string, skipping ANSI escape sequences.
fn visible_width(s: &str) -> usize {
    let mut w = 0;
    let mut in_esc = false;
    for c in s.chars() {
        if in_esc {
            if c.is_ascii_alphabetic() {
                in_esc = false;
            }
        } else if c == '\x1b' {
            in_esc = true;
        } else {
            w += 1;
        }
    }
    w
}

/// Help text, shown beside the logo on a wide enough terminal. Style tags:
/// `$h` heading, `$b` bold, `$a` argument, `$y` value, `$d` dim, `$0` reset.
const HELP_TOP: &str = "\
$bhype$0 {VERSION} \u{2014} images as high-fidelity terminal text art, fast

$hUsage:$0 $bhype$0 $a<IMAGE>$0 [OPTIONS]
       $bhype$0 $b--layer$0 $a<IMAGE>$0... [OPTIONS]

$hOptions:$0
  $b-w$0, $b--width$0 $a<N>$0       Width in columns $d(default: fit -H, else terminal width)$0
  $b-H$0, $b--height$0 $a<N>$0      Height in rows $d(default: keep the image's aspect ratio)$0
  $b-m$0, $b--mode$0 $a<MODE>$0     $yblock$0, $ybraille$0, $yascii$0 $d(default: block)$0
  $b-c$0, $b--color$0 $a<MODE>$0    $ytrue$0, $y256$0, $ygray$0 $d(default: auto-detected, here {COLOR})$0
  $b-d$0, $b--dither$0 $a<TYPE>$0   $yfs$0, $yordered$0, $ynone$0 $d(default: none; 256-color block only)$0
  $b-t$0, $b--threshold$0 $a<N>$0   Braille: dots fainter than N stay unlit $d(default: 40)$0
  $b-b$0, $b--bg$0 $a<COLOR>$0      Flatten transparency onto $yblack$0, $ywhite$0 or $yR,G,B$0
  $b--light$0               Draw for a light background $d(light theme, or paper)$0
  $b--text$0 [$a<FILE>$0]       Draw the image with this text $d(stdin if - or none; implies ascii)$0
  $b--svg$0 [$a<FIT>$0]         Vector SVG output $d(fit: fill or natural; default fill)$0
  $b--tint$0 [$a<N>$0]          SVG: tint cells to carry tone glyphs can't $d(0-100, default 50)$0
  $b-l$0, $b--layer$0           Overlay further images, in order, on the first
  $b-h$0, $b--help$0, $bhelp$0      Show this help
  $b-v$0, $b--version$0         Show version

$hModes:$0
  $yblock$0     Half blocks and quadrants, two colors per cell. Highest fidelity.
  $ybraille$0   2x4 dots per cell in one color; unlit dots show the background.
  $yascii$0     Printable ASCII only: density ramp, shape-matched at edges.
            With --text, the file's text in reading order instead, one
            character per non-blank cell, the image carried by color.
";

const HELP_REST: &str = "
$hOutput contract$0 $d(for scripts and agents)$0
  - stdout carries only the image: one line per row, each exactly W columns
    wide (UTF-8 with ANSI SGR color codes), then a final newline. No color
    state carries over between lines, so lines can be used individually.
  - Size: -w or -H alone sets that side and the other follows the image's
    aspect ratio (cells are about twice as tall as wide); neither: terminal
    width (80 if unknown). Both -w and -H given: stretch.
  - With --layer, the first image sets the size; each later one is scaled
    to fit inside it, keeping its aspect ratio, centered, and drawn over
    the layers before it.
  - With -w and -c given, output is deterministic for the same image.
  - $b-c gray$0 in braille or ascii mode emits no escape codes at all.
  - Transparent areas show the terminal background unless -b is given.
  - The terminal background is assumed dark, or light with --light. Braille
    and ascii leave areas close to it blank (\"fainter\" in -t is measured
    from it) and draw the rest in the image's colors.
  - --text collapses whitespace to single spaces, maps typographic quotes,
    dashes and ellipses to ASCII, drops other non-ASCII, and repeats the
    text if the image outlasts it. Underscores marking italics, as in
    Project Gutenberg's _word_, are dropped (a paragraph break ends them);
    --svg draws those words in italics.
  - --svg ({SVG}) outputs one SVG document, then a final newline:
    ascii (the default) or braille, each cell 1x2 units, blank cells left
    as the background (black, or white with --light). Glyphs are from the
    Recursive font (c) The Recursive Project Authors, SIL Open Font
    License 1.1. fill enlarges them to fill their cells; natural keeps the
    font's proportions. Full color unless -c is given; -c gray draws all
    in one ink color. --tint N fills each drawn cell (and --text word
    spaces) behind its glyph with the tone the glyph lacks, at most N% of
    the ink's depth from the background, so the image reads from a
    distance; gray tints are gray.
  - Errors: one line \"hype: <message>\" on stderr and exit status 1.
    A reader closing the pipe early is not an error.

$hFormats:$0     {FORMATS} $d(this build)$0
$hEnvironment:$0 COLORTERM=truecolor or 24bit enables 24-bit color by default;
             NO_COLOR gives plain help text (image output is unaffected).

$hExamples:$0
  $bhype$0 logo.png                     $d# fit the terminal width$0
  $bhype$0 icon.png -w 32 -H 16         $d# exactly 32x16 cells$0
  $bhype$0 logo.png -w 60 -c 256        $d# for a 256-color terminal$0
  $bhype$0 logo.png -m ascii -c gray    $d# plain ASCII, no escape codes$0
  $bhype$0 logo.png -w 80 > logo.ans    $d# save now, show later with cat$0
  $bhype$0 -l frame.png icon.png        $d# icon centered over frame$0
  $bhype$0 logo.png --text chapter.txt  $d# the image drawn in a book's words$0
  $bfortune$0 | $bhype$0 logo.png --text    $d# ...or in piped text$0
  $bhype$0 photo.jpg -w 200 --svg --light > print.svg  $d# vector art to print$0
";

fn print_help(color: ColorMode, light: bool) {
    // Styling and the logo are for humans at a terminal; captured output
    // (an agent or script) gets the same text without escape codes.
    let fancy = term::is_tty(1) && std::env::var_os("NO_COLOR").is_none();
    let formats: Vec<&str> = [
        ("PNG", cfg!(feature = "png")),
        ("JPEG", cfg!(feature = "jpeg")),
        ("GIF", cfg!(feature = "gif")),
        ("WebP", cfg!(feature = "webp")),
        ("BMP", cfg!(feature = "bmp")),
    ]
    .into_iter()
    .filter_map(|(name, on)| on.then_some(name))
    .collect();
    let detected = match detect_color_support() {
        ColorMode::True => "true",
        _ => "256",
    };
    let style = |text: &str| {
        let mut s = text
            .replace("{VERSION}", VERSION)
            .replace("{COLOR}", detected)
            .replace("{FORMATS}", &formats.join(", "))
            .replace(
                "{SVG}",
                if cfg!(feature = "svg") {
                    "in this build"
                } else {
                    "not in this build"
                },
            );
        for (tag, esc) in [
            ("$h", "\x1b[1;32m"),
            ("$b", "\x1b[1m"),
            ("$a", "\x1b[32m"),
            ("$y", "\x1b[33m"),
            ("$d", "\x1b[2m"),
            ("$0", "\x1b[0m"),
        ] {
            s = s.replace(tag, if fancy { esc } else { "" });
        }
        s
    };
    let help = style(HELP_TOP) + &style(HELP_REST);
    let bd = render::Backdrop { bg: None, light };

    let lines: Vec<&str> = help.lines().collect();
    let help_width = lines.iter().map(|l| visible_width(l)).max().unwrap_or(0);
    let gap = 4;
    let room = term::width().unwrap_or(80).saturating_sub(help_width + gap);

    let mut out = String::new();
    match fancy.then(|| logo(room, lines.len(), color, bd)).flatten() {
        Some(logo) => {
            // The logo is never taller than the help; center it beside it.
            let art: Vec<&str> = logo.lines().collect();
            let offset = (lines.len() - art.len()) / 2;
            for (i, line) in lines.iter().enumerate() {
                let pad = help_width + gap - visible_width(line);
                let art = i.checked_sub(offset).and_then(|j| art.get(j));
                out.push_str(&format!("{line}{:pad$}{}\n", "", art.unwrap_or(&"")));
            }
        }
        None => out.push_str(&help),
    }
    emit(out.as_bytes());
}

/// The logo, rendered by hype itself as large as fits beside the help text:
/// `cols` wide, unless that would be more than `rows` tall. None if there is
/// too little room or this build lacks PNG.
fn logo(cols: usize, rows: usize, color: ColorMode, bd: render::Backdrop) -> Option<String> {
    if !cfg!(feature = "png") || cols < 16 {
        return None;
    }
    let mut img = decode::from_bytes(LOGO, |w, h| {
        let size = target_size(Some(cols), None, 0, w, h);
        if size.1 / 4 <= rows {
            size
        } else {
            target_size(None, Some(rows), 0, w, h)
        }
    })
    .ok()?;
    let (w, h) = (img.width, img.height);
    let mut out = Vec::new();
    render::render_block(&mut img.pixels, w, h, color, Dither::None, bd, &mut out);
    String::from_utf8(out).ok()
}

/// Write to stdout. A consumer closing the pipe early is not an error worth
/// reporting.
fn emit(bytes: &[u8]) {
    if let Err(e) = std::io::stdout().lock().write_all(bytes)
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        eprintln!("hype: {e}");
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_args_image_only() {
        let a = args(&["photo.png"]);
        let opts = parse_args(&a, ColorMode::True).unwrap();
        assert_eq!(opts.images, ["photo.png"]);
        assert!(!opts.help);
        assert!(!opts.version);
        assert!(!opts.light);
    }

    #[test]
    fn parse_args_all_flags() {
        let a = args(&[
            "img.jpg", "-w", "120", "-H", "40", "-m", "braille", "-c", "256", "-d", "fs", "-t",
            "100", "-b", "white", "--light",
        ]);
        let opts = parse_args(&a, ColorMode::True).unwrap();
        assert!(opts.light);
        assert_eq!(opts.width, Some(120));
        assert_eq!(opts.height, Some(40));
        assert!(matches!(opts.mode, Mode::Braille));
        assert_eq!(opts.color, ColorMode::Ansi256);
        assert_eq!(opts.dither, Dither::FloydSteinberg);
        assert_eq!(opts.threshold, 100);
        assert_eq!(opts.bg, Some((255, 255, 255)));
    }

    #[test]
    fn parse_args_help_and_version() {
        let a = args(&["-h"]);
        assert!(parse_args(&a, ColorMode::True).unwrap().help);

        let a = args(&["--version"]);
        assert!(parse_args(&a, ColorMode::True).unwrap().version);
    }

    fn expect_err(a: &[&str], needle: &str) {
        match parse_args(&args(a), ColorMode::True) {
            Err(e) => assert!(e.contains(needle), "expected '{needle}' in error: {e}"),
            Ok(_) => panic!("expected error containing '{needle}'"),
        }
    }

    #[test]
    fn parse_args_unknown_option() {
        expect_err(&["--bogus"], "unknown option");
    }

    #[test]
    fn parse_args_missing_value() {
        expect_err(&["-w"], "missing value");
    }

    #[test]
    fn parse_args_invalid_mode() {
        expect_err(&["-m", "pixel"], "unknown mode");
    }

    #[test]
    fn parse_args_duplicate_positional() {
        expect_err(&["a.png", "b.png"], "unexpected argument");
    }

    #[test]
    fn parse_args_layer() {
        // The flag may come before or after the images; order is kept.
        for a in [
            &["-l", "a.png", "b.png", "c.png"][..],
            &["a.png", "b.png", "c.png", "--layer"],
        ] {
            let opts = parse_args(&args(a), ColorMode::True).unwrap();
            assert_eq!(opts.images, ["a.png", "b.png", "c.png"]);
        }
    }

    #[test]
    fn parse_args_text_implies_ascii() {
        for a in [
            &["img.png", "--text", "t.txt"][..],
            &["--text", "t.txt", "-m", "ascii", "img.png"],
        ] {
            let opts = parse_args(&args(a), ColorMode::True).unwrap();
            assert!(matches!(opts.mode, Mode::Ascii));
            assert_eq!(opts.text.as_deref(), Some("t.txt"));
        }
        expect_err(
            &["img.png", "--text", "t.txt", "-m", "block"],
            "ascii mode only",
        );
        // No file, or "-": stdin.
        for a in [
            &["img.png", "--text"][..],
            &["img.png", "--text", "-w", "40"],
            &["--text", "-", "img.png"],
        ] {
            let opts = parse_args(&args(a), ColorMode::True).unwrap();
            assert_eq!(opts.text.as_deref(), Some("-"));
            assert_eq!(opts.images, ["img.png"]);
        }
    }

    #[cfg(feature = "svg")]
    #[test]
    fn parse_args_svg() {
        let opts = parse_args(&args(&["img.png", "--svg"]), ColorMode::Ansi256).unwrap();
        assert_eq!(opts.svg, Some(Fit::Fill));
        // Ascii by default, and full color rather than the terminal's.
        assert!(opts.mode == Mode::Ascii);
        assert_eq!(opts.color, ColorMode::True);
        // The fit is taken only when it is one; anything else stays.
        let opts = parse_args(&args(&["--svg", "natural", "img.png"]), ColorMode::True).unwrap();
        assert_eq!(opts.svg, Some(Fit::Natural));
        assert_eq!(opts.images, ["img.png"]);
        let a = args(&["--svg", "img.png", "-m", "braille", "-c", "gray"]);
        let opts = parse_args(&a, ColorMode::True).unwrap();
        assert_eq!(opts.svg, Some(Fit::Fill));
        assert!(opts.mode == Mode::Braille);
        assert_eq!(opts.color, ColorMode::Gray);
        expect_err(&["img.png", "--svg", "-m", "block"], "not block");
        // --tint: off unless given, 50 without a number.
        assert_eq!(opts.tint, 0);
        let opts = parse_args(&args(&["--svg", "--tint", "img.png"]), ColorMode::True).unwrap();
        assert_eq!(
            (opts.tint, &opts.images[..]),
            (50, &["img.png".to_string()][..])
        );
        let opts = parse_args(&args(&["--svg", "--tint", "30", "x.png"]), ColorMode::True).unwrap();
        assert_eq!(opts.tint, 30);
        expect_err(&["img.png", "--svg", "--tint", "101"], "0-100");
        expect_err(&["img.png", "--tint"], "--svg only");
    }

    #[cfg(not(feature = "svg"))]
    #[test]
    fn parse_args_svg_needs_feature() {
        expect_err(&["img.png", "--svg"], "not in this build");
    }

    #[test]
    fn target_size_sides() {
        // 2:1 image: 40 columns need 10 rows (cells are twice as tall).
        assert_eq!(target_size(None, None, 40, 200, 100), (80, 40));
        assert_eq!(target_size(Some(40), None, 80, 200, 100), (80, 40));
        // Height alone keeps the aspect ratio rather than taking the default width.
        assert_eq!(target_size(None, Some(10), 80, 200, 100), (80, 40));
        // Both: stretch.
        assert_eq!(target_size(Some(7), Some(3), 80, 200, 100), (14, 12));
    }

    #[test]
    fn fit_size_keeps_aspect() {
        // Wide image in a square canvas: full width, centered vertically.
        assert_eq!(fit_size(100, 100, 400, 200), (100, 50));
        // Tall image: full height.
        assert_eq!(fit_size(100, 100, 100, 400), (25, 100));
        // Same aspect ratio: fills the canvas.
        assert_eq!(fit_size(80, 40, 1000, 500), (80, 40));
        // Never collapses to zero.
        assert_eq!(fit_size(10, 10, 10000, 1), (10, 1));
    }

    #[test]
    fn parse_args_threshold_out_of_range() {
        expect_err(&["-t", "300"], "threshold");
    }

    #[test]
    fn parse_args_default_color_mode_preserved() {
        let a = args(&["img.png"]);
        let opts = parse_args(&a, ColorMode::Ansi256).unwrap();
        assert_eq!(opts.color, ColorMode::Ansi256);
    }

    #[test]
    fn parse_bg_named() {
        assert_eq!(parse_bg("black"), Ok((0, 0, 0)));
        assert_eq!(parse_bg("white"), Ok((255, 255, 255)));
    }

    #[test]
    fn parse_bg_rgb() {
        assert_eq!(parse_bg("10,20,30"), Ok((10, 20, 30)));
        assert_eq!(parse_bg("0,0,0"), Ok((0, 0, 0)));
        assert_eq!(parse_bg("255,255,255"), Ok((255, 255, 255)));
    }

    #[test]
    fn parse_bg_invalid() {
        assert!(parse_bg("red").is_err());
        assert!(parse_bg("10,20").is_err());
        assert!(parse_bg("10,20,300").is_err());
        assert!(parse_bg("").is_err());
    }
}
