# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Hype is a terminal image viewer written in Rust (edition 2024). It renders images as colored text using half-block, braille, or ASCII characters with support for truecolor, 256-color, and grayscale output.

## Build & Run

```bash
cargo build              # debug build
cargo build --release    # release build
cargo run -- <image> [options]  # run directly
```

No tests or linter are configured.

## Architecture

Three source files in `src/`:

- **main.rs** — CLI argument parsing (hand-rolled, no clap), image loading via the `image` crate, resize/aspect-ratio logic, and dispatch to render functions. Rendering mode (`Block`/`Braille`/`Ascii`) and color mode (`True`/`Ansi256`/`Gray`) are determined here.
- **render.rs** — All rendering logic. Each mode has its own public function (`render_block`, `render_braille`, `render_ascii`). Handles ANSI escape code generation, alpha compositing via `apply_bg`, and dithering (Floyd-Steinberg, ordered Bayer 4×4) for 256-color block mode.
- **color.rs** — Color math utilities: BT.601 luminance, ANSI 256-color nearest-match (6×6×6 cube + grayscale ramp), perceptually-weighted color distance, and alpha blending.

Key design points:
- Half-block mode packs two vertical pixels per character cell using `▀`/`▄` with fg/bg colors.
- Braille mode maps 2×4 pixel blocks to Unicode braille characters (U+2800–U+28FF), coloring each cell by average color of lit dots.
- Dithering only applies to 256-color block mode; it quantizes to the 6-level cube before rendering.
- Terminal color support is auto-detected from `COLORTERM` env var, falling back to 256-color.
