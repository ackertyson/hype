mod color;
mod render;

use image::GenericImageView;
use render::{ColorMode, Dither, Pixel};
use std::process;

const VERSION: &str = env!("CARGO_PKG_VERSION");

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
        print_help();
        return;
    }
    if opts.version {
        println!("hype {VERSION}");
        return;
    }

    let path = match &opts.image {
        Some(p) => p.clone(),
        None => {
            eprintln!("hype: no image specified (use -h for help)");
            process::exit(1);
        }
    };

    let img = match image::open(&path) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("hype: cannot open '{}': {}", path, e);
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
    if let Ok(ct) = std::env::var("COLORTERM") {
        if ct == "truecolor" || ct == "24bit" {
            return ColorMode::True;
        }
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
            "-h" | "--help" => opts.help = true,
            "-V" | "--version" => opts.version = true,
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

fn print_help() {
    println!(
        "\
hype {VERSION} — terminal image art generator

USAGE: hype <IMAGE> [OPTIONS]

Options:
  -w, --width <N>       Output width in columns (default: terminal width)
  -H, --height <N>      Output height in rows (default: auto from aspect ratio)
  -m, --mode <MODE>     block, braille, ascii (default: block)
  -c, --color <MODE>    true, 256, gray (default: true)
  -d, --dither <TYPE>   fs, ordered, none (default: none)
  -t, --threshold <N>   Braille brightness threshold 0-255 (default: 40)
  -b, --bg <COLOR>      Alpha background: black, white, or R,G,B (default: transparent)
  -h, --help            Show help
  -V, --version         Show version"
    );
}
