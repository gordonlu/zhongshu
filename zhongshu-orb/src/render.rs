/// Procedural orb renderer — Siri‑inspired glowing sphere in a plain
/// `[u32]` buffer (0xAARRGGBB).  Zero GPU, zero assets, just math.

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum OrbMode {
    Idle,
    Thinking,
    Executing,
    Done,
}

/// Draw an orb into `buf` (row‑major 0xAARRGGBB pixels, size ww×hh).
#[allow(dead_code)]
pub fn draw_orb(buf: &mut [u32], ww: u32, hh: u32, cr: u8, cg: u8, cb: u8, t: f64, mode: OrbMode) {
    let small = ww.min(hh) < 64;

    let (cx, cy) = (ww as f64 / 2.0, hh as f64 / 2.0);
    let max_r = (ww.min(hh) as f64 / 2.0) - 1.0;

    // Breath: radius oscillates ±8 %.
    let period = match mode {
        OrbMode::Idle => 2.5,
        OrbMode::Thinking => 1.4,
        OrbMode::Executing => 0.6,
        OrbMode::Done => 2.0,
    };
    let breath = 1.0 + (t * std::f64::consts::TAU / period).sin() * 0.08;
    let outer = max_r * breath;

    // Glow falloff: secondary larger layer.
    let glow_r = outer * 1.6;

    // Highlight: off‑centre specular.
    let (hl_x, hl_y) = (cx - outer * 0.25, cy - outer * 0.25);

    // Energy waves (active states only, skip for small sizes).
    let wave_active = !small && matches!(mode, OrbMode::Thinking | OrbMode::Executing);
    let wave_count = if wave_active { 2 } else { 0 };

    // Colour flow (slow hue shift for active states).
    let hue_shift = if matches!(mode, OrbMode::Thinking | OrbMode::Executing) {
        (t * 0.15).sin() * 0.12
    } else {
        0.0
    };
    let (r, g, b) = shift_hue(cr, cg, cb, hue_shift);

    for y in 0..hh {
        for x in 0..ww {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let dist = (dx * dx + dy * dy).sqrt();

            // ── Glow layer ───────────────────────────────────────────
            let mut alpha = 0.0;

            if !small && dist < glow_r {
                let gf = dist / glow_r;
                alpha = 1.0 - gf * gf; // quadratic falloff
                alpha *= 0.25; // glow is semi‑transparent
            }

            // ── Outer sphere ─────────────────────────────────────────
            if dist < outer {
                let n = dist / outer; // 0 … 1
                let inner = 0.55;
                let core = if n < inner {
                    1.0 // fully opaque core
                } else {
                    // Gradient band
                    let band = (n - inner) / (1.0 - inner);
                    1.0 - band * band * (3.0 - 2.0 * band) // smoothstep
                };
                alpha = alpha.max(core);
            }

            // ── Highlight (skip for very small sizes) ────────────────
            if !small {
                let hdx = x as f64 - hl_x;
                let hdy = y as f64 - hl_y;
                let hdist = (hdx * hdx + hdy * hdy).sqrt();
                let hl_radius = outer * 0.35;
                if hdist < hl_radius {
                    let hn = hdist / hl_radius;
                    let hl_alpha = 1.0 - hn * hn;
                    let a = (alpha * 255.0) as u32;
                    let rp = ((r as f64 * alpha * 255.0 + 200.0 * hl_alpha * 0.5) as u32).min(255);
                    let gp = ((g as f64 * alpha * 255.0 + 200.0 * hl_alpha * 0.5) as u32).min(255);
                    let bp = ((b as f64 * alpha * 255.0 + 220.0 * hl_alpha * 0.5) as u32).min(255);
                    buf[(y * ww + x) as usize] = (a << 24) | (rp << 16) | (gp << 8) | bp;
                    continue;
                }
            }

            // ── Energy waves ─────────────────────────────────────────
            if wave_active && dist < outer * 1.5 && dist > outer * 0.2 {
                for wi in 0..wave_count {
                    let wave_phase = (t * 1.5 + wi as f64 * 0.5).fract();
                    let wave_r = outer * (0.3 + wave_phase * 0.9);
                    let thickness = outer * 0.06;
                    let wdist = (dist - wave_r).abs();
                    if wdist < thickness {
                        let wn = wdist / thickness;
                        let w_alpha = (1.0 - wn * wn) * (0.15 + 0.1 * (t.sin()));
                        alpha = alpha.max(w_alpha);
                    }
                }
            }

            let a = (alpha * 255.0).min(255.0) as u32;
            if a == 0 {
                continue;
            }
            // Premultiply colour by alpha for correct blending.
            let aa = a as f64 / 255.0;
            let rp = (r as f64 * aa) as u32;
            let gp = (g as f64 * aa) as u32;
            let bp = (b as f64 * aa) as u32;
            buf[(y * ww + x) as usize] = (a << 24) | (rp << 16) | (gp << 8) | bp;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_ppm(path: &str, buf: &[u32], w: u32, h: u32) {
        let mut data = Vec::with_capacity((w * h * 3) as usize);
        for &pixel in buf {
            let a = (pixel >> 24) & 0xFF;
            let r = (pixel >> 16) & 0xFF;
            let g = (pixel >> 8) & 0xFF;
            let b = pixel & 0xFF;
            // Premultiplied alpha → straight alpha for viewing.
            let scale = if a == 0 { 0.0 } else { 255.0 / a as f64 };
            data.push((r as f64 * scale) as u8);
            data.push((g as f64 * scale) as u8);
            data.push((b as f64 * scale) as u8);
        }
        let header = format!("P6\n{w} {h}\n255\n");
        let mut out = std::fs::File::create(path).unwrap();
        use std::io::Write;
        out.write_all(header.as_bytes()).unwrap();
        out.write_all(&data).unwrap();
    }

    #[test]
    fn orb_renders_all_modes_to_ppm() {
        let (ww, hh) = (128u32, 128u32);
        let mut buf = vec![0u32; (ww * hh) as usize];
        let base_dir = std::env::temp_dir().join("zhongshu_orb_test");
        let _ = std::fs::create_dir_all(&base_dir);

        for (mode_name, mode) in &[
            ("idle", OrbMode::Idle),
            ("thinking", OrbMode::Thinking),
            ("executing", OrbMode::Executing),
            ("done", OrbMode::Done),
        ] {
            for (phase_name, t) in &[("t0", 0.0), ("t_peak", 1.0), ("t_dip", 0.5)] {
                buf.fill(0);
                draw_orb(&mut buf, ww, hh, 100, 140, 255, *t, *mode);
                let path = base_dir.join(format!("orb_{mode_name}_{phase_name}.ppm"));
                write_ppm(path.to_str().unwrap(), &buf, ww, hh);
            }
        }
        // Verify centre pixel is lit.
        buf.fill(0);
        draw_orb(&mut buf, ww, hh, 100, 140, 255, 0.0, OrbMode::Idle);
        let centre = buf[hh as usize / 2 * ww as usize + ww as usize / 2];
        assert_ne!(centre & 0x00FFFFFF, 0, "centre pixel should have colour");

        // Verify centre is much brighter than a corner (soft glow reaches
        // edges by design — the glow radius is 1.6× the sphere radius).
        let centre_alpha = (centre >> 24) as u32;
        let corner_alpha = (buf[0] >> 24) as u32;
        assert!(
            centre_alpha > corner_alpha * 10,
            "centre alpha ({centre_alpha}) should be much higher than corner ({corner_alpha})"
        );

        eprintln!("Orb PPM frames written to {}/", base_dir.display());
    }

    #[test]
    fn orb_breathing_changes_radius() {
        let (ww, hh) = (64u32, 64u32);
        let mut buf = vec![0u32; (ww * hh) as usize];
        let max_r = (ww.min(hh) as f64 / 2.0) - 1.0;

        // At peak (t = period/4 = 0.625), radius = max_r × 1.08.
        draw_orb(&mut buf, ww, hh, 100, 140, 255, 0.625, OrbMode::Idle);
        // Pixel just inside the peak radius should be lit.
        let inner = ((max_r * 1.07) as u32).min(ww / 2 - 1);
        let idx = (hh / 2 * ww + ww / 2 + inner) as usize;
        assert_ne!(buf[idx] >> 24, 0, "pixel at peak radius edge should be lit");

        // At dip (t = period × 3/4 = 1.875), radius = max_r × 0.92.
        buf.fill(0);
        draw_orb(&mut buf, ww, hh, 100, 140, 255, 1.875, OrbMode::Idle);
        // Same pixel should now be outside the sphere (darker or transparent).
        let dip_a = buf[idx] >> 24;
        let peak_a = {
            let mut b = vec![0u32; (ww * hh) as usize];
            draw_orb(&mut b, ww, hh, 100, 140, 255, 0.625, OrbMode::Idle);
            b[idx] >> 24
        };
        assert!(
            dip_a < peak_a,
            "breathing: alpha at radius edge should be lower at dip ({dip_a}) than peak ({peak_a})"
        );
    }
}

/// Shift an sRGB colour's hue by `amount` (‑0.5 … 0.5) via a cheap
/// rotation in a pseudo‑HSV space.
#[allow(dead_code)]
fn shift_hue(r: u8, g: u8, b: u8, amount: f64) -> (u8, u8, u8) {
    if amount.abs() < 0.001 {
        return (r, g, b);
    }
    let (rf, gf, bf) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let cos_a = amount.cos();
    let sin_a = amount.sin();
    // Simple rotation around (1,1,1) axis – preserves luminance approximately.
    let nr = (rf * (0.333 + 0.667 * cos_a)
        + gf * (0.333 - 0.333 * cos_a + 0.577 * sin_a)
        + bf * (0.333 - 0.333 * cos_a - 0.577 * sin_a))
        .clamp(0.0, 1.0);
    let ng = (rf * (0.333 - 0.333 * cos_a - 0.577 * sin_a)
        + gf * (0.333 + 0.667 * cos_a)
        + bf * (0.333 - 0.333 * cos_a + 0.577 * sin_a))
        .clamp(0.0, 1.0);
    let nb = (rf * (0.333 - 0.333 * cos_a + 0.577 * sin_a)
        + gf * (0.333 - 0.333 * cos_a - 0.577 * sin_a)
        + bf * (0.333 + 0.667 * cos_a))
        .clamp(0.0, 1.0);
    ((nr * 255.0) as u8, (ng * 255.0) as u8, (nb * 255.0) as u8)
}
