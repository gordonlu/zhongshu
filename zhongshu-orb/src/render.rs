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

    // Energy waves (active states only).
    let wave_active = matches!(mode, OrbMode::Thinking | OrbMode::Executing);
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

            if dist < glow_r {
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

            // ── Highlight ────────────────────────────────────────────
            let hdx = x as f64 - hl_x;
            let hdy = y as f64 - hl_y;
            let hdist = (hdx * hdx + hdy * hdy).sqrt();
            let hl_radius = outer * 0.35;
            if hdist < hl_radius {
                let hn = hdist / hl_radius;
                let hl_alpha = 1.0 - hn * hn;
                // Blend highlight (additive, white-ish).
                let a = (alpha * 255.0) as u32;
                let rp = ((r as f64 * alpha * 255.0 + 200.0 * hl_alpha * 0.5) as u32).min(255);
                let gp = ((g as f64 * alpha * 255.0 + 200.0 * hl_alpha * 0.5) as u32).min(255);
                let bp = ((b as f64 * alpha * 255.0 + 220.0 * hl_alpha * 0.5) as u32).min(255);
                let a = a.min(255);
                buf[(y * ww + x) as usize] = (a << 24) | (rp << 16) | (gp << 8) | bp;
                continue;
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
