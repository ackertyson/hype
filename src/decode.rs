use crate::render::Pixel;
use crate::resize::Resampler;

/// A decoded image, already resampled to the requested output size.
pub struct Image {
    pub pixels: Vec<Pixel>,
    pub width: usize,
    pub height: usize,
}

/// Load an image and resample it to the size chosen by `size`, which is
/// called with the source dimensions once the header has been read.
pub fn load(path: &str, size: impl Fn(usize, usize) -> (usize, usize)) -> Result<Image, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    from_bytes(&data, size)
}

/// Like `load`, for an image already in memory.
pub fn from_bytes(
    data: &[u8],
    size: impl Fn(usize, usize) -> (usize, usize),
) -> Result<Image, String> {
    let mut sink = Sink {
        size: &size,
        r: None,
    };

    match &data[..data.len().min(12)] {
        [0x89, b'P', b'N', b'G', ..] => png(data, &mut sink)?,
        [0xff, 0xd8, 0xff, ..] => jpeg(data, &mut sink)?,
        [b'G', b'I', b'F', b'8', ..] => gif(data, &mut sink)?,
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P'] => webp(data, &mut sink)?,
        [b'B', b'M', ..] => bmp(data, &mut sink)?,
        _ => return Err("unsupported image format".into()),
    }

    match sink.r {
        Some((r, w, h)) => Ok(Image {
            pixels: r.finish(),
            width: w,
            height: h,
        }),
        None => Err("image contains no pixel data".into()),
    }
}

/// Receives decoded rows and forwards them, as RGBA8, to the resampler.
struct Sink<'a> {
    size: &'a dyn Fn(usize, usize) -> (usize, usize),
    r: Option<(Resampler, usize, usize)>,
}

impl Sink<'_> {
    fn begin(&mut self, w: usize, h: usize) -> Result<(), String> {
        if w == 0 || h == 0 {
            return Err("image has zero size".into());
        }
        let (ow, oh) = (self.size)(w, h);
        self.r = Some((Resampler::new(w, h, ow, oh), ow, oh));
        Ok(())
    }

    /// Push one row with `ch` interleaved channels (1 gray, 2 gray+alpha,
    /// 3 RGB, 4 RGBA), using `buf` as scratch space for the RGBA expansion.
    fn row(&mut self, src: &[u8], ch: usize, buf: &mut Vec<u8>) {
        let Some((r, _, _)) = self.r.as_mut() else {
            return;
        };
        if ch == 4 {
            r.push_row(src);
            return;
        }
        buf.clear();
        for p in src.chunks_exact(ch) {
            buf.extend_from_slice(&match ch {
                1 => [p[0], p[0], p[0], 255],
                2 => [p[0], p[0], p[0], p[1]],
                _ => [p[0], p[1], p[2], 255],
            });
        }
        r.push_row(buf);
    }

    fn rows(&mut self, data: &[u8], w: usize, ch: usize) {
        let mut buf = Vec::with_capacity(w * 4);
        for row in data.chunks_exact(w * ch) {
            self.row(row, ch, &mut buf);
        }
    }
}

#[allow(dead_code)]
fn disabled(name: &str) -> Result<(), String> {
    Err(format!(
        "{name} support not enabled (rebuild with --features {name})"
    ))
}

#[cfg(feature = "png")]
fn png(data: &[u8], sink: &mut Sink) -> Result<(), String> {
    use png::{ColorType, Transformations};

    let err = |e: png::DecodingError| e.to_string();
    let mut dec = png::Decoder::new_with_limits(
        std::io::Cursor::new(data),
        png::Limits { bytes: usize::MAX },
    );
    dec.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(err)?;
    let (w, h, interlaced) = {
        let info = reader.info();
        (info.width as usize, info.height as usize, info.interlaced)
    };
    let ch = match reader.output_color_type().0 {
        ColorType::Grayscale => 1,
        ColorType::GrayscaleAlpha => 2,
        ColorType::Rgb => 3,
        ColorType::Rgba => 4,
        ColorType::Indexed => return Err("unexpanded indexed PNG".into()),
    };
    sink.begin(w, h)?;

    if interlaced {
        // Adam7 passes arrive out of order; decode the whole frame first.
        let mut frame = vec![0; reader.output_buffer_size().ok_or("PNG too large")?];
        reader.next_frame(&mut frame).map_err(err)?;
        sink.rows(&frame, w, ch);
    } else {
        let mut buf = Vec::with_capacity(w * 4);
        while let Some(row) = reader.next_row().map_err(err)? {
            sink.row(row.data(), ch, &mut buf);
        }
    }
    Ok(())
}

#[cfg(not(feature = "png"))]
fn png(_: &[u8], _: &mut Sink) -> Result<(), String> {
    disabled("png")
}

#[cfg(feature = "jpeg")]
fn jpeg(data: &[u8], sink: &mut Sink) -> Result<(), String> {
    use zune_core::bytestream::ZCursor;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;
    use zune_jpeg::JpegDecoder;

    let err = |e: zune_jpeg::errors::DecodeErrors| e.to_string();
    let opts = DecoderOptions::default()
        .set_max_width(usize::MAX)
        .set_max_height(usize::MAX);
    let mut dec = JpegDecoder::new_with_options(ZCursor::new(data), opts);
    dec.decode_headers().map_err(err)?;
    let (w, h) = dec.dimensions().ok_or("missing JPEG dimensions")?;
    let out = match dec.input_colorspace().ok_or("missing JPEG colorspace")? {
        cs @ (ColorSpace::Luma | ColorSpace::LumaA | ColorSpace::RGB | ColorSpace::RGBA) => cs,
        _ => ColorSpace::RGB,
    };
    dec.set_options(dec.options().jpeg_set_out_colorspace(out));
    let pixels = dec.decode().map_err(err)?;
    sink.begin(w, h)?;
    sink.rows(&pixels, w, out.num_components());
    Ok(())
}

#[cfg(not(feature = "jpeg"))]
fn jpeg(_: &[u8], _: &mut Sink) -> Result<(), String> {
    disabled("jpeg")
}

/// GIF: renders the first frame onto the logical screen.
#[cfg(feature = "gif")]
fn gif(data: &[u8], sink: &mut Sink) -> Result<(), String> {
    let err = |e: gif::DecodingError| e.to_string();
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::RGBA);
    let mut dec = opts.read_info(data).map_err(err)?;
    let (sw, sh) = (dec.width() as usize, dec.height() as usize);
    let frame = dec
        .read_next_frame()
        .map_err(err)?
        .ok_or("GIF has no frames")?;
    let (fl, ft) = (frame.left as usize, frame.top as usize);
    let (fw, fh) = (frame.width as usize, frame.height as usize);

    let mut canvas = vec![0u8; sw * sh * 4];
    for y in 0..fh.min(sh.saturating_sub(ft)) {
        let n = fw.min(sw.saturating_sub(fl)) * 4;
        let src = &frame.buffer[y * fw * 4..][..n];
        let dst = ((ft + y) * sw + fl) * 4;
        canvas[dst..dst + n].copy_from_slice(src);
    }
    sink.begin(sw, sh)?;
    sink.rows(&canvas, sw, 4);
    Ok(())
}

#[cfg(not(feature = "gif"))]
fn gif(_: &[u8], _: &mut Sink) -> Result<(), String> {
    disabled("gif")
}

#[cfg(feature = "webp")]
fn webp(data: &[u8], sink: &mut Sink) -> Result<(), String> {
    let err = |e: image_webp::DecodingError| e.to_string();
    let mut dec = image_webp::WebPDecoder::new(std::io::Cursor::new(data)).map_err(err)?;
    let (w, h) = dec.dimensions();
    let ch = if dec.has_alpha() { 4 } else { 3 };
    let mut buf = vec![0; dec.output_buffer_size().ok_or("WebP too large")?];
    dec.read_image(&mut buf).map_err(err)?;
    sink.begin(w as usize, h as usize)?;
    sink.rows(&buf, w as usize, ch);
    Ok(())
}

#[cfg(not(feature = "webp"))]
fn webp(_: &[u8], _: &mut Sink) -> Result<(), String> {
    disabled("webp")
}

/// Uncompressed BMP: 1/4/8-bit palette, 16/24/32-bit RGB and bitfields.
#[cfg(feature = "bmp")]
fn bmp(data: &[u8], sink: &mut Sink) -> Result<(), String> {
    let short = || "truncated BMP".to_string();
    let u16_at = |o: usize| -> Result<u32, String> {
        let b = data.get(o..o + 2).ok_or_else(short)?;
        Ok(u16::from_le_bytes([b[0], b[1]]) as u32)
    };
    let u32_at = |o: usize| -> Result<u32, String> {
        let b = data.get(o..o + 4).ok_or_else(short)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };

    let pixel_off = u32_at(10)? as usize;
    let hdr = u32_at(14)? as usize;
    let (w, h, bpp, comp, colors, pal_entry) = if hdr == 12 {
        let w = u16_at(18)? as i32;
        let h = u16_at(20)? as i16 as i32;
        (w, h, u16_at(24)?, 0, 0, 3)
    } else if hdr >= 40 {
        let w = u32_at(18)? as i32;
        let h = u32_at(22)? as i32;
        (w, h, u16_at(28)?, u32_at(30)?, u32_at(46)?, 4)
    } else {
        return Err(format!("unsupported BMP header size {hdr}"));
    };
    if w <= 0 || h == 0 {
        return Err("invalid BMP dimensions".into());
    }
    let (w, top_down) = (w as usize, h < 0);
    let h = h.unsigned_abs() as usize;

    // Channel masks: explicit for BITFIELDS (3) / ALPHABITFIELDS (6).
    let mut masks = match bpp {
        16 => [0x7c00, 0x03e0, 0x001f, 0],
        _ => [0xff0000, 0x00ff00, 0x0000ff, 0],
    };
    let mut pal_off = 14 + hdr;
    match comp {
        0 => {}
        1 if bpp == 8 => {}
        2 if bpp == 4 => {}
        3 | 6 if bpp == 16 || bpp == 32 => {
            // Masks follow a 40-byte header, or live inside a larger one.
            let base = if hdr == 40 { 14 + hdr } else { 54 };
            let n = if comp == 6 || hdr >= 56 { 4 } else { 3 };
            for (i, m) in masks.iter_mut().enumerate().take(n) {
                *m = u32_at(base + i * 4)?;
            }
            if hdr == 40 {
                pal_off += n * 4;
            }
        }
        _ => return Err(format!("unsupported BMP compression {comp}")),
    }

    let palette: Vec<[u8; 3]> = if bpp <= 8 {
        let n = if colors == 0 {
            1 << bpp
        } else {
            colors as usize
        };
        (0..n.min(256))
            .map(|i| {
                let o = pal_off + i * pal_entry;
                data.get(o..o + 3)
                    .map(|b| [b[2], b[1], b[0]])
                    .ok_or_else(short)
            })
            .collect::<Result<_, _>>()?
    } else {
        Vec::new()
    };

    if comp == 1 || comp == 2 {
        let src = data.get(pixel_off..).ok_or_else(short)?;
        let rows = bmp_rle(src, w, h, top_down, bpp, &palette);
        sink.begin(w, h)?;
        sink.rows(&rows, w, 4);
        return Ok(());
    }

    let channel = |px: u32, mask: u32| -> u8 {
        if mask == 0 {
            return 255;
        }
        let bits = mask.count_ones();
        let v = (px & mask) >> mask.trailing_zeros();
        ((v * 255 + ((1 << bits) - 1) / 2) / ((1 << bits) - 1)) as u8
    };
    let alpha_mask = masks[3];

    let stride = (w * bpp as usize).div_ceil(32) * 4;
    if !matches!(bpp, 1 | 4 | 8 | 16 | 24 | 32) {
        return Err(format!("unsupported BMP bit depth {bpp}"));
    }
    stride
        .checked_mul(h)
        .and_then(|n| data.get(pixel_off..pixel_off.checked_add(n)?))
        .ok_or_else(short)?;

    sink.begin(w, h)?;
    let mut out = Vec::with_capacity(w * 4);
    for y in 0..h {
        let sy = if top_down { y } else { h - 1 - y };
        let row = &data[pixel_off + sy * stride..][..stride];
        out.clear();
        for x in 0..w {
            let rgba = match bpp {
                1 | 4 | 8 => {
                    let bit = x * bpp as usize;
                    let byte = row[bit / 8];
                    let shift = 8 - bpp as usize - bit % 8;
                    let idx = (byte >> shift) as usize & ((1 << bpp) - 1);
                    let [r, g, b] = palette.get(idx).copied().unwrap_or([0, 0, 0]);
                    [r, g, b, 255]
                }
                24 => [row[x * 3 + 2], row[x * 3 + 1], row[x * 3], 255],
                _ => {
                    let px = if bpp == 16 {
                        u16::from_le_bytes([row[x * 2], row[x * 2 + 1]]) as u32
                    } else {
                        let b = &row[x * 4..x * 4 + 4];
                        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
                    };
                    [
                        channel(px, masks[0]),
                        channel(px, masks[1]),
                        channel(px, masks[2]),
                        channel(px, alpha_mask),
                    ]
                }
            };
            out.extend_from_slice(&rgba);
        }
        sink.row(&out, 4, &mut Vec::new());
    }
    Ok(())
}

/// Decode RLE8 (`bpp` 8) or RLE4 (`bpp` 4) into RGBA rows, top to bottom.
/// Pixels skipped by delta or early end-of-bitmap codes stay transparent,
/// and truncated data simply ends the image.
#[cfg(feature = "bmp")]
fn bmp_rle(
    src: &[u8],
    w: usize,
    h: usize,
    top_down: bool,
    bpp: u32,
    palette: &[[u8; 3]],
) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    let (mut x, mut y, mut i) = (0usize, 0usize, 0usize);
    let mut put = |x: usize, y: usize, idx: u8| {
        if x < w && y < h {
            let [r, g, b] = palette.get(idx as usize).copied().unwrap_or([0, 0, 0]);
            let row = if top_down { y } else { h - 1 - y };
            out[(row * w + x) * 4..][..4].copy_from_slice(&[r, g, b, 255]);
        }
    };
    // In RLE4 each byte holds two pixels, high nibble first.
    let nib = |byte: u8, k: usize| {
        if k.is_multiple_of(2) {
            byte >> 4
        } else {
            byte & 0x0f
        }
    };

    while i + 1 < src.len() && y < h {
        let (n, v) = (src[i] as usize, src[i + 1]);
        i += 2;
        match (n, v) {
            (0, 0) => (x, y) = (0, y + 1),
            (0, 1) => break,
            (0, 2) => {
                let Some(d) = src.get(i..i + 2) else { break };
                (x, y) = (x + d[0] as usize, y + d[1] as usize);
                i += 2;
            }
            (0, n) => {
                // Absolute run of `n` pixels, padded to a 16-bit boundary.
                let n = n as usize;
                let bytes = if bpp == 8 { n } else { n.div_ceil(2) };
                let Some(run) = src.get(i..i + bytes) else {
                    break;
                };
                for k in 0..n {
                    let idx = if bpp == 8 { run[k] } else { nib(run[k / 2], k) };
                    put(x, y, idx);
                    x += 1;
                }
                i += bytes + bytes % 2;
            }
            (n, v) => {
                for k in 0..n {
                    put(x, y, if bpp == 8 { v } else { nib(v, k) });
                    x += 1;
                }
            }
        }
    }
    out
}

#[cfg(not(feature = "bmp"))]
fn bmp(_: &[u8], _: &mut Sink) -> Result<(), String> {
    disabled("bmp")
}
