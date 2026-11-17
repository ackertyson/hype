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
mod render;
mod resize;
mod term;

use render::{ColorMode, Dither};
use std::io::Write;
use std::process;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const LOGO_256: &str = include_str!("../logo_256.txt");
const LOGO_TRUE: &str = include_str!("../logo_truecolor.txt");

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
        print_help(opts.color);
        return;
    }
    if opts.version {
        emit(format!("hype {VERSION}\n").as_bytes());
        return;
    }

    let path = opts.image.as_deref().unwrap_or_else(|| {
        eprintln!("hype: no image specified (use -h for help)");
        process::exit(1);
    });

    let term_width = opts.width.unwrap_or_else(|| term::width().unwrap_or(80));
    let size = |img_w: usize, img_h: usize| target_size(term_width, opts.height, img_w, img_h);

    let mut img = match decode::load(path, size) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("hype: cannot open '{path}': {e}");
            process::exit(1);
        }
    };
    let (w, h) = (img.width, img.height);

    let mut out = Vec::with_capacity(w * h * 12);
    let px = &mut img.pixels;
    match opts.mode {
        Mode::Block => render::render_block(px, w, h, opts.color, opts.dither, opts.bg, &mut out),
        Mode::Braille => {
            render::render_braille(px, w, h, opts.color, opts.threshold, opts.bg, &mut out)
        }
        Mode::Ascii => render::render_ascii(px, w, h, opts.color, opts.bg, &mut out),
    }
    out.push(b'\n');

    emit(&out);
}

/// Resampled pixel dimensions: every mode samples each character cell as
/// 2×4 subpixels. Cells are about twice as tall as wide, so an image needs
/// half as many rows as its aspect ratio implies.
fn target_size(cols: usize, height: Option<usize>, img_w: usize, img_h: usize) -> (usize, usize) {
    let cols = cols.max(1);
    let rows = height
        .unwrap_or_else(|| ((cols as f64 * img_h as f64 / img_w as f64) as usize).div_ceil(2))
        .max(1);
    (cols * 2, rows * 4)
}

#[derive(Clone, Copy)]
enum Mode {
    Block,
    Braille,
    Ascii,
}

struct Opts {
    image: Option<String>,
    width: Option<usize>,
    height: Option<usize>,
    mode: Mode,
    color: ColorMode,
    dither: Dither,
    threshold: u8,
    bg: Option<(u8, u8, u8)>,
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
        image: None,
        width: None,
        height: None,
        mode: Mode::Block,
        color: default_color,
        dither: Dither::None,
        threshold: 40,
        bg: None,
        help: false,
        version: false,
    };

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" | "help" => opts.help = true,
            "-v" | "--version" => opts.version = true,
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
                opts.mode = match val.as_str() {
                    "block" => Mode::Block,
                    "braille" => Mode::Braille,
                    "ascii" => Mode::Ascii,
                    _ => return Err(format!("unknown mode '{val}' (block, braille, ascii)")),
                };
            }
            "-c" | "--color" => {
                i += 1;
                let val = next_arg(args, i, "color")?;
                opts.color = match val.as_str() {
                    "true" => ColorMode::True,
                    "256" => ColorMode::Ansi256,
                    "gray" | "grey" => ColorMode::Gray,
                    _ => return Err(format!("unknown color mode '{val}' (true, 256, gray)")),
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
            other => {
                if other.starts_with('-') {
                    return Err(format!("unknown option '{other}'"));
                }
                if opts.image.is_none() {
                    opts.image = Some(other.to_string());
                } else {
                    return Err(format!("unexpected argument '{other}'"));
                }
            }
        }
        i += 1;
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

$hOptions:$0
  $b-w$0, $b--width$0 $a<N>$0       Width in columns $d(default: terminal width, 80 if none)$0
  $b-H$0, $b--height$0 $a<N>$0      Height in rows $d(default: keep the image's aspect ratio)$0
  $b-m$0, $b--mode$0 $a<MODE>$0     $yblock$0, $ybraille$0, $yascii$0 $d(default: block)$0
  $b-c$0, $b--color$0 $a<MODE>$0    $ytrue$0, $y256$0, $ygray$0 $d(default: auto-detected, here {COLOR})$0
  $b-d$0, $b--dither$0 $a<TYPE>$0   $yfs$0, $yordered$0, $ynone$0 $d(default: none; 256-color block only)$0
  $b-t$0, $b--threshold$0 $a<N>$0   Braille: dots dimmer than N stay unlit $d(default: 40)$0
  $b-b$0, $b--bg$0 $a<COLOR>$0      Flatten transparency onto $yblack$0, $ywhite$0 or $yR,G,B$0
  $b-h$0, $b--help$0, $bhelp$0      Show this help
  $b-v$0, $b--version$0         Show version

$hModes:$0
  $yblock$0     Half blocks and quadrants, two colors per cell. Highest fidelity.
  $ybraille$0   2x4 dots per cell in one color; unlit dots show the background.
  $yascii$0     Printable ASCII only: density ramp, shape-matched at edges.
";

const HELP_REST: &str = "
$hOutput contract$0 $d(for scripts and agents)$0
  - stdout carries only the image: one line per row, each exactly W columns
    wide (UTF-8 with ANSI SGR color codes), then a final newline. No color
    state carries over between lines, so lines can be used individually.
  - Rows come from -H, else from the width and the image's aspect ratio
    (cells are about twice as tall as wide). Both -w and -H given: stretch.
  - With -w and -c given, output is deterministic for the same image.
  - $b-c gray$0 in braille or ascii mode emits no escape codes at all.
  - Transparent areas show the terminal background unless -b is given.
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
";

fn print_help(color: ColorMode) {
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
            .replace("{FORMATS}", &formats.join(", "));
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
    let (top, rest) = (style(HELP_TOP), style(HELP_REST));

    let logo: Vec<&str> = if color == ColorMode::True {
        LOGO_TRUE
    } else {
        LOGO_256
    }
    .lines()
    .collect();
    let logo_width = logo.iter().map(|l| visible_width(l)).max().unwrap_or(0);
    let lines: Vec<&str> = top.lines().collect();
    let help_width = lines.iter().map(|l| visible_width(l)).max().unwrap_or(0);
    let gap = 4;

    let mut out = String::new();
    if fancy && term::width().unwrap_or(80) >= help_width + gap + logo_width {
        let total = lines.len().max(logo.len());
        let logo_offset = total.saturating_sub(logo.len()) / 2;
        for i in 0..total {
            let help = lines.get(i).copied().unwrap_or("");
            let art = i
                .checked_sub(logo_offset)
                .and_then(|j| logo.get(j))
                .copied()
                .unwrap_or("");
            let pad = help_width + gap - visible_width(help);
            out.push_str(&format!("{help}{:pad$}{art}\x1b[0m\n", ""));
        }
    } else {
        out.push_str(&top);
    }
    out.push_str(&rest);
    emit(out.as_bytes());
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
        assert_eq!(opts.image.as_deref(), Some("photo.png"));
        assert!(!opts.help);
        assert!(!opts.version);
    }

    #[test]
    fn parse_args_all_flags() {
        let a = args(&[
            "img.jpg", "-w", "120", "-H", "40", "-m", "braille", "-c", "256", "-d", "fs", "-t",
            "100", "-b", "white",
        ]);
        let opts = parse_args(&a, ColorMode::True).unwrap();
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
