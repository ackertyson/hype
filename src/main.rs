mod color;
mod render;

use image::GenericImageView;
use render::{ColorMode, Dither, Pixel};
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
        println!("hype {VERSION}");
        return;
    }

    let path = opts.image.as_deref().unwrap_or_else(|| {
        eprintln!("hype: no image specified (use -h for help)");
        process::exit(1);
    });

    let img = match image::open(path) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("hype: cannot open '{path}': {e}");
            process::exit(1);
        }
    };

    let term_width = opts.width.unwrap_or_else(|| {
        terminal_size::terminal_size()
            .map(|(w, _)| w.0 as usize)
            .unwrap_or(80)
    });

    let (img_w, img_h) = img.dimensions();
    let aspect = img_h as f64 / img_w as f64;

    let (resize_w, resize_h) = match opts.mode {
        Mode::Block => {
            let cols = term_width;
            let rows = opts
                .height
                .map(|h| h * 2)
                .unwrap_or_else(|| (cols as f64 * aspect) as usize);
            // Ensure even height
            let rows = rows + (rows % 2);
            (cols as u32, rows as u32)
        }
        Mode::Braille => {
            let cols = term_width;
            let dot_w = cols * 2;
            let dot_h = opts
                .height
                .map(|h| h * 4)
                .unwrap_or_else(|| (dot_w as f64 * aspect) as usize);
            // Round to multiples of 4 and 2
            let dot_h = dot_h.div_ceil(4) * 4;
            let dot_w = dot_w.div_ceil(2) * 2;
            (dot_w as u32, dot_h as u32)
        }
        Mode::Ascii => {
            let cols = term_width;
            // Terminal chars are ~2:1 tall, so halve height
            let rows = opts.height.unwrap_or((cols as f64 * aspect * 0.5) as usize);
            (cols as u32, rows as u32)
        }
    };

    let resized = img.resize_exact(resize_w, resize_h, image::imageops::FilterType::Lanczos3);
    let rgba = resized.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);

    let pixels: Vec<Pixel> = rgba
        .pixels()
        .map(|p| Pixel {
            r: p[0],
            g: p[1],
            b: p[2],
            a: p[3],
        })
        .collect();

    let output = match opts.mode {
        Mode::Block => render::render_block(&pixels, w, h, opts.color, opts.dither, opts.bg),
        Mode::Braille => render::render_braille(&pixels, w, h, opts.color, opts.threshold, opts.bg),
        Mode::Ascii => render::render_ascii(&pixels, w, h, opts.color, opts.bg),
    };

    println!("{output}");
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

fn print_help(color: ColorMode) {
    const GREEN: &str = "\x1b[32m";
    const YELLOW: &str = "\x1b[33m";
    const DIM: &str = "\x1b[2m";
    const R: &str = "\x1b[0m";
    const BG: &str = "\x1b[1;32m";
    const BW: &str = "\x1b[1m";

    let lines = vec![
        format!("hype v{VERSION} \u{2014} terminal text art generator"),
        String::new(),
        format!("{BG}Usage:{R} {BW}hype{R} {GREEN}<IMAGE>{R} [OPTIONS]"),
        String::new(),
        format!("{BG}Options:{R}"),
        format!(
            "  {BW}-w{R}, {BW}--width{R} {GREEN}<N>{R}       Output width in columns {DIM}(default: terminal width){R}"
        ),
        format!(
            "  {BW}-H{R}, {BW}--height{R} {GREEN}<N>{R}      Output height in rows {DIM}(default: auto from aspect ratio){R}"
        ),
        format!(
            "  {BW}-m{R}, {BW}--mode{R} {GREEN}<MODE>{R}     block, braille, ascii {DIM}(default: block){R}"
        ),
        format!(
            "  {BW}-c{R}, {BW}--color{R} {GREEN}<MODE>{R}    true, 256, gray {DIM}(default: true){R}"
        ),
        format!(
            "  {BW}-d{R}, {BW}--dither{R} {GREEN}<TYPE>{R}   fs, ordered, none {DIM}(default: none; 256-color block-mode only){R}"
        ),
        format!(
            "  {BW}-t{R}, {BW}--threshold{R} {GREEN}<N>{R}   Braille brightness threshold 0\u{2013}255 {DIM}(default: 40){R}"
        ),
        format!(
            "  {BW}-b{R}, {BW}--bg{R} {GREEN}<COLOR>{R}      Alpha background: {YELLOW}black{R}, {YELLOW}white{R}, or {YELLOW}R,G,B{R} {DIM}(default: transparent){R}"
        ),
        format!("  {BW}-h{R}, {BW}--help{R}, {BW}help{R}      Show help"),
        format!("  {BW}-v{R}, {BW}--version{R}         Show version"),
    ];

    let logo = if color == ColorMode::True {
        LOGO_TRUE
    } else {
        LOGO_256
    };
    let logo_lines: Vec<&str> = logo.lines().collect();
    let logo_width = logo_lines
        .iter()
        .map(|l| visible_width(l))
        .max()
        .unwrap_or(0);

    let gap = 4;
    let help_width = lines.iter().map(|l| visible_width(l)).max().unwrap_or(0);
    let needed = help_width + gap + logo_width;
    let tw = terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(80);

    if tw >= needed && !logo_lines.is_empty() {
        let total = lines.len().max(logo_lines.len());
        let help_offset = 0;
        let logo_offset = total.saturating_sub(logo_lines.len()) / 2;

        for i in 0..total {
            let help_part = if i >= help_offset && i - help_offset < lines.len() {
                &lines[i - help_offset]
            } else {
                ""
            };
            let logo_part = if i >= logo_offset && i - logo_offset < logo_lines.len() {
                logo_lines[i - logo_offset]
            } else {
                ""
            };
            let pad = help_width + gap - visible_width(help_part);
            println!("{}{:pad$}{}\x1b[0m", help_part, "", logo_part, pad = pad);
        }
    } else {
        for line in &lines {
            println!("{line}");
        }
    }
}
