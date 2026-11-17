# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Hype is a terminal text art generator written in Rust (edition 2024). It renders images as colored text using half-block, braille, or ASCII characters with support for truecolor, 256-color, and grayscale output.

## Build & Run

```bash
cargo fmt                # format code (run after changes)
cargo clippy             # lint (run after changes, fix any warnings; CI uses -D warnings per feature)
cargo build              # debug build
cargo build --release    # release build
cargo test                # run tests
cargo deny check         # dependency policy: advisories, licenses, sources, bans (deny.toml)
cargo run -- <image> [options]  # run directly
```

Releasing: `build/tag.sh X.Y.Z` (on an up-to-date master) bumps `Cargo.toml`, commits, tags and pushes. The tag runs `.github/workflows/release.yml`, which builds static binaries (macOS arm64/x86_64, Linux musl arm64/x86_64) and attaches them with `checksums.txt` to a GitHub release. Then run `release.sh X.Y.Z` in the `homebrew-hype` tap to point the formula at them.

## Architecture

Source files in `src/`:

- **main.rs** — CLI argument parsing (hand-rolled, no clap), output sizing (`target_size`), and dispatch to render functions. Rendering mode (`Block`/`Braille`/`Ascii`) and color mode (`True`/`Ansi256`/`Gray`) are determined here. Output is built into one byte buffer and written once.
- **decode.rs** — Format sniffing by magic bytes and decoders that stream RGBA rows into the resampler. PNG (`png` crate) and JPEG (`zune-jpeg`) are default features; GIF (`gif`), WebP (`image-webp`) and BMP (hand-rolled, incl. RLE4/8) are opt-in cargo features.
- **resize.rs** — Streaming resampler: exact integer box pre-reduction (keeping a ≥2× "reducing gap") followed by separable Lanczos3, on premultiplied alpha. The full-size source image is never held in memory (except JPEG, interlaced PNG, GIF and WebP, whose decoders produce whole frames).
- **render.rs** — All rendering logic. Each mode has its own public function (`render_block`, `render_braille`, `render_ascii`) writing into a `Vec<u8>`. `Sgr` tracks terminal color state and emits only changes, combining fg/bg into one escape. Also alpha compositing (`apply_bg`), dithering (Floyd-Steinberg, ordered Bayer 4×4) for 256-color block mode, and the measured ASCII glyph coverage table (`GLYPHS`).
- **color.rs** — Color math utilities: integer BT.601 luminance (`lum1000`), ANSI 256-color nearest-match (6×6×6 cube + grayscale ramp), perceptually-weighted color distance, and alpha blending.
- **term.rs** — Terminal width via a direct `ioctl(TIOCGWINSZ)` FFI call (stdout, then stderr, then stdin), and `isatty`. Unix only.

Key design points:
- Goal: minimal resource cost per invocation (startup, RSS, binary size) — hype is meant to be shelled out to on demand by other terminal apps. Prefer brutally simple code; add complexity only for a measured win. Release profile uses fat LTO, 1 codegen unit, `panic = "abort"`, strip, opt-level 3 (`"s"` measured ~2× slower for ~80 KB savings).
- Every mode samples each character cell as 2×4 subpixels (`cell()`), so `target_size` is mode-independent.
- Block mode fits each cell: of the seven two-region half-block/quadrant shapes (`SHAPES`), pick the one whose side means, as the terminal will display them (`Sgr::shown`, so 256-color is fit in-palette), give the least perceptual error. Uniform cells become a background-colored space; the glyph or its complement is chosen to minimize color changes. Partly transparent cells draw the shape nearest the opaque subpixels over the default background. Without `--bg`, translucent pixels are first faded toward black (the assumed dark terminal), which keeps silhouette edges antialiased.
- Braille and ASCII have one color per cell over the (assumed dark) terminal background, and see translucent pixels composited over black. `lit_mask` decides which subpixels to draw: above the threshold, and better represented by the lit pixels' mean color than by black. Braille lights exactly those dots; ASCII treats a cell the mask splits as an edge and picks the shape-matched glyph, else uses the density ramp on mean luminance.
- Dithering only applies to 256-color block mode; it averages cells down to plain half-block pixels, quantizes to the 6-level cube, and renders with `▀`/`▄`.
- Each output line ends with no color state set, so lines stand alone when consumers slice output. `--help` documents this output contract for agents; keep it accurate. Help styling and the logo appear only when stdout is a TTY and `NO_COLOR` is unset.
- Terminal color support is auto-detected from `COLORTERM` env var, falling back to 256-color.
