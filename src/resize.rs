use crate::render::Pixel;

/// Streaming two-stage resampler: an exact integer box pre-reduction
/// followed by a separable Lanczos3 filter.
///
/// Input rows are fed top to bottom with `push_row`, so the full-size source
/// image never has to be held in memory. When shrinking by a large factor,
/// source pixels are first box-averaged by a whole factor that still leaves
/// the Lanczos stage a ≥2× reduction (the "reducing gap" approach), which is
/// visually indistinguishable from pure Lanczos but far cheaper. Filtering is
/// done on premultiplied alpha to avoid dark or colored fringes around
/// transparent edges.
pub struct Resampler {
    in_w: usize,
    in_h: usize,
    kx: usize,
    ky: usize,
    /// Box sums of premultiplied color (×255) and alpha for the current band.
    bins: Vec<[u32; 4]>,
    /// Source rows accumulated into `bins` so far.
    band: usize,
    y: usize,
    lanczos: Lanczos,
}

/// Minimum reduction left for the Lanczos stage after box pre-reduction.
const REDUCING_GAP: usize = 2;

impl Resampler {
    pub fn new(in_w: usize, in_h: usize, out_w: usize, out_h: usize) -> Self {
        // Capped so a bin's sum (≤ 255·255 per pixel) always fits in a u32.
        let kx = (in_w / (out_w * REDUCING_GAP)).clamp(1, 256);
        let ky = (in_h / (out_h * REDUCING_GAP)).clamp(1, 256);
        let (bw, bh) = (in_w.div_ceil(kx), in_h.div_ceil(ky));
        Resampler {
            in_w,
            in_h,
            kx,
            ky,
            bins: vec![[0; 4]; bw],
            band: 0,
            y: 0,
            lanczos: Lanczos::new(bw, bh, out_w, out_h),
        }
    }

    /// Feed the next source row, in RGBA8 order.
    pub fn push_row(&mut self, rgba: &[u8]) {
        if self.y >= self.in_h {
            return;
        }
        self.y += 1;

        for (bin, chunk) in self.bins.iter_mut().zip(rgba.chunks(self.kx * 4)) {
            for p in chunk.chunks_exact(4) {
                let a = p[3] as u32;
                bin[0] += p[0] as u32 * a;
                bin[1] += p[1] as u32 * a;
                bin[2] += p[2] as u32 * a;
                bin[3] += a;
            }
        }
        self.band += 1;

        if self.band == self.ky || self.y == self.in_h {
            self.flush_band();
        }
    }

    fn flush_band(&mut self) {
        let row = self.lanczos.input_row();
        let last_w = self.in_w - (self.bins.len() - 1) * self.kx;
        for (i, (d, s)) in row.iter_mut().zip(&self.bins).enumerate() {
            let w = if i + 1 == self.bins.len() {
                last_w
            } else {
                self.kx
            };
            let n = (w * self.band) as f32;
            let c = 1.0 / (n * 255.0);
            *d = [
                s[0] as f32 * c,
                s[1] as f32 * c,
                s[2] as f32 * c,
                s[3] as f32 / n,
            ];
        }
        self.lanczos.push_input_row();
        self.bins.iter_mut().for_each(|b| *b = [0; 4]);
        self.band = 0;
    }

    pub fn finish(self) -> Vec<Pixel> {
        self.lanczos.finish()
    }
}

/// Streaming separable Lanczos3 on premultiplied f32 rows: each row is
/// filtered horizontally and immediately accumulated into every output row
/// it contributes to.
struct Lanczos {
    out_w: usize,
    out_h: usize,
    h: Weights,
    v: Weights,
    row: Vec<[f32; 4]>,
    hrow: Vec<[f32; 4]>,
    acc: Vec<[f32; 4]>,
    y: usize,
    first: usize,
}

/// Filter weights for one axis, flattened to `taps` entries per output sample.
struct Weights {
    start: Vec<usize>,
    len: Vec<usize>,
    taps: usize,
    w: Vec<f32>,
}

fn lanczos3(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    if x.abs() >= 3.0 {
        return 0.0;
    }
    let px = std::f64::consts::PI * x;
    3.0 * px.sin() * (px / 3.0).sin() / (px * px)
}

impl Weights {
    fn new(in_n: usize, out_n: usize) -> Self {
        let scale = in_n as f64 / out_n as f64;
        // Widen the kernel when downscaling so every source pixel contributes.
        let fscale = scale.max(1.0);
        let support = 3.0 * fscale;

        let mut start = Vec::with_capacity(out_n);
        let mut len = Vec::with_capacity(out_n);
        let mut rows: Vec<Vec<f32>> = Vec::with_capacity(out_n);
        for i in 0..out_n {
            let center = (i as f64 + 0.5) * scale;
            let lo = (center - support).floor().max(0.0) as usize;
            let hi = ((center + support).ceil() as usize).clamp(lo + 1, in_n);
            let mut ws: Vec<f64> = (lo..hi)
                .map(|j| lanczos3((j as f64 + 0.5 - center) / fscale))
                .collect();
            let sum: f64 = ws.iter().sum();
            if sum.abs() > 1e-12 {
                ws.iter_mut().for_each(|w| *w /= sum);
            }
            start.push(lo);
            len.push(hi - lo);
            rows.push(ws.into_iter().map(|w| w as f32).collect());
        }

        // Rounded up to whole groups of 4 for the unrolled inner loop.
        let taps = len.iter().copied().max().unwrap_or(1).next_multiple_of(4);
        let mut w = vec![0.0; out_n * taps];
        for (i, ws) in rows.iter().enumerate() {
            w[i * taps..i * taps + ws.len()].copy_from_slice(ws);
        }
        Weights {
            start,
            len,
            taps,
            w,
        }
    }
}

impl Lanczos {
    fn new(in_w: usize, in_h: usize, out_w: usize, out_h: usize) -> Self {
        let h = Weights::new(in_w, out_w);
        let v = Weights::new(in_h, out_h);
        Lanczos {
            out_w,
            out_h,
            // Padded so every output sample can read a full `taps` window.
            row: vec![[0.0; 4]; in_w + h.taps],
            hrow: vec![[0.0; 4]; out_w],
            acc: vec![[0.0; 4]; out_w * out_h],
            h,
            v,
            y: 0,
            first: 0,
        }
    }

    /// The buffer to fill with the next premultiplied input row.
    fn input_row(&mut self) -> &mut [[f32; 4]] {
        let n = self.row.len() - self.h.taps;
        &mut self.row[..n]
    }

    fn push_input_row(&mut self) {
        let y = self.y;
        self.y += 1;

        // Skip output rows whose window ends above this input row.
        while self.first < self.out_h && self.v.start[self.first] + self.v.len[self.first] <= y {
            self.first += 1;
        }
        let mut last = self.first;
        while last < self.out_h && self.v.start[last] <= y {
            last += 1;
        }
        if last == self.first {
            return;
        }

        let taps = self.h.taps;
        for (x, o) in self.hrow.iter_mut().enumerate() {
            let s = self.h.start[x];
            let ws = &self.h.w[x * taps..(x + 1) * taps];
            // Four independent accumulators break the FMA dependency chain.
            let mut acc = [[0.0f32; 4]; 4];
            for (ps, ks) in self.row[s..s + taps]
                .chunks_exact(4)
                .zip(ws.chunks_exact(4))
            {
                for i in 0..4 {
                    for c in 0..4 {
                        acc[i][c] += ps[i][c] * ks[i];
                    }
                }
            }
            *o = std::array::from_fn(|c| (acc[0][c] + acc[1][c]) + (acc[2][c] + acc[3][c]));
        }

        let vtaps = self.v.taps;
        for j in self.first..last {
            let off = y - self.v.start[j];
            if off >= self.v.len[j] {
                continue;
            }
            let k = self.v.w[j * vtaps + off];
            let dst = &mut self.acc[j * self.out_w..(j + 1) * self.out_w];
            for (d, s) in dst.iter_mut().zip(&self.hrow) {
                for c in 0..4 {
                    d[c] += s[c] * k;
                }
            }
        }
    }

    /// Un-premultiply and quantize the accumulated output.
    fn finish(self) -> Vec<Pixel> {
        self.acc
            .iter()
            .map(|p| {
                let a = p[3].clamp(0.0, 255.0);
                if a < 0.5 {
                    return Pixel {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 0,
                    };
                }
                let inv = 255.0 / a;
                let q = |v: f32| (v * inv).clamp(0.0, 255.0).round() as u8;
                Pixel {
                    r: q(p[0]),
                    g: q(p[1]),
                    b: q(p[2]),
                    a: a.round() as u8,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(w: usize, h: usize, ow: usize, oh: usize, px: [u8; 4]) -> Vec<Pixel> {
        let mut r = Resampler::new(w, h, ow, oh);
        let row: Vec<u8> = px.iter().copied().cycle().take(w * 4).collect();
        for _ in 0..h {
            r.push_row(&row);
        }
        r.finish()
    }

    #[test]
    fn solid_color_survives_downscale() {
        for p in run(64, 48, 10, 7, [200, 100, 50, 255]) {
            assert_eq!((p.r, p.g, p.b, p.a), (200, 100, 50, 255));
        }
    }

    #[test]
    fn solid_color_survives_box_prereduction() {
        // 1000/10 leaves a box factor of 33 with a ragged last bin.
        let mut r = Resampler::new(1000, 999, 10, 9);
        assert!(r.kx > 1 && r.ky > 1);
        let row: Vec<u8> = [30, 60, 90, 128].repeat(1000);
        for _ in 0..999 {
            r.push_row(&row);
        }
        for p in r.finish() {
            assert_eq!((p.r, p.g, p.b, p.a), (30, 60, 90, 128));
        }
    }

    #[test]
    fn solid_color_survives_upscale() {
        for p in run(3, 2, 17, 9, [10, 220, 90, 255]) {
            assert_eq!((p.r, p.g, p.b, p.a), (10, 220, 90, 255));
        }
    }

    #[test]
    fn identity_size_is_lossless() {
        let (w, h) = (5, 4);
        let src: Vec<u8> = (0..w * h * 4).map(|i| (i * 13 % 256) as u8 | 1).collect();
        let mut r = Resampler::new(w, h, w, h);
        for row in src.chunks_exact(w * 4) {
            r.push_row(row);
        }
        let out = r.finish();
        for (i, p) in out.iter().enumerate() {
            let s = &src[i * 4..i * 4 + 4];
            assert_eq!(p.a, s[3]);
            // Un-premultiplying a low alpha can be off by a little.
            for (a, b) in [(p.r, s[0]), (p.g, s[1]), (p.b, s[2])] {
                assert!((a as i16 - b as i16).abs() <= 2, "{a} vs {b}");
            }
        }
    }

    #[test]
    fn transparent_pixels_do_not_bleed_color() {
        // Opaque red next to fully transparent "white": edge must stay red.
        let mut r = Resampler::new(4, 1, 2, 1);
        r.push_row(&[
            255, 0, 0, 255, 255, 0, 0, 255, 255, 255, 255, 0, 255, 255, 255, 0,
        ]);
        let out = r.finish();
        assert!(
            out[0].g < 8 && out[0].b < 8,
            "fringe: {:?}",
            (out[0].g, out[0].b)
        );
    }
}
