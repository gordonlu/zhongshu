#![allow(dead_code)]

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrbMode {
    Idle,
    Thinking,
    Executing,
    Done,
}

/// Draw a glowing orb with breathing animation.
///
/// Layers (back to front):
///   1. transparent background
///   2. outer glow ring (optional, Thinking/Executing only)
///   3. gradient body (bright center → fade to edge)
///   4. solid bright core
///
/// `r/g/b` — current state color (already interpolated by caller)
/// `t` — elapsed time in seconds (for breathing phase)
/// `mode` — per-state behavior (breath speed, glow intensity)
pub fn draw_orb(pixels: &mut [u32], w: u32, h: u32, r: u8, g: u8, b: u8, t: f64, mode: OrbMode) {
    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;
    let base_r = (w.min(h) as f32 / 2.0) - 3.0;

    fill_rect(pixels, pack(0, 0, 0, 0));

    // Breathing parameters per state
    let (period, amp) = match mode {
        OrbMode::Idle => (2.0, 0.03),
        OrbMode::Thinking => (1.2, 0.06),
        OrbMode::Executing => (0.5, 0.10),
        OrbMode::Done => (2.0, 0.02),
    };
    let breath = 1.0 + ((t * std::f64::consts::TAU / period).sin() as f32) * amp;
    let radius = base_r * breath;

    // Outer glow (Thinking / Executing get an extra aura ring)
    match mode {
        OrbMode::Thinking | OrbMode::Executing => {
            fill_circle_gradient(pixels, w, cx, cy, base_r * 1.4, (r, g, b), 0.12);
        }
        _ => {}
    }

    // Main gradient body
    let intensity = match mode {
        OrbMode::Idle => 0.5,
        OrbMode::Thinking => 0.7,
        OrbMode::Executing => 0.8,
        OrbMode::Done => 0.5,
    };
    fill_circle_gradient(pixels, w, cx, cy, radius, (r, g, b), intensity);

    // Bright solid core
    let core_radius = radius
        * match mode {
            OrbMode::Idle => 0.4,
            OrbMode::Thinking => 0.5,
            OrbMode::Executing => 0.6,
            OrbMode::Done => 0.4,
        };
    fill_circle_solid(pixels, w, cx, cy, core_radius, pack(r, g, b, 255));
}

// ── Pixel helpers ─────────────────────────────────────────────────────

fn pack(r: u8, g: u8, b: u8, a: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

fn unpack(buf: u32) -> (u8, u8, u8, u8) {
    let a = ((buf >> 24) & 0xff) as u8;
    let r = ((buf >> 16) & 0xff) as u8;
    let g = ((buf >> 8) & 0xff) as u8;
    let b = (buf & 0xff) as u8;
    (r, g, b, a)
}

fn blend_over(dst: u32, r: u8, g: u8, b: u8, a: u8) -> u32 {
    if a == 0 {
        return dst;
    }
    if a == 255 {
        return pack(r, g, b, 255);
    }

    let (dr, dg, db, da) = unpack(dst);
    let sa = a as f32 / 255.0;
    let da_f = da as f32 / 255.0;

    let out_a = sa + da_f * (1.0 - sa);
    if out_a < 0.001 {
        return dst;
    }

    let out_r = ((r as f32 * sa + dr as f32 * da_f * (1.0 - sa)) / out_a) as u8;
    let out_g = ((g as f32 * sa + dg as f32 * da_f * (1.0 - sa)) / out_a) as u8;
    let out_b = ((b as f32 * sa + db as f32 * da_f * (1.0 - sa)) / out_a) as u8;
    let out_a_u8 = (out_a * 255.0) as u8;
    pack(out_r, out_g, out_b, out_a_u8)
}

fn fill_rect(buf: &mut [u32], color: u32) {
    buf.fill(color);
}

fn fill_circle_solid(buf: &mut [u32], w: u32, cx: f32, cy: f32, r: f32, color: u32) {
    let ir = r.ceil() as i32;
    let r2 = r * r;
    for dy in -ir..=ir {
        for dx in -ir..=ir {
            if (dx * dx + dy * dy) as f32 > r2 {
                continue;
            }
            let px = (cx as i32 + dx) as u32;
            let py = (cy as i32 + dy) as u32;
            if px < w && py < buf.len() as u32 / w {
                buf[(py * w + px) as usize] = color;
            }
        }
    }
}

fn fill_circle_gradient(buf: &mut [u32], w: u32, cx: f32, cy: f32, r: f32, base: (u8, u8, u8), intensity: f32) {
    let (rr, gg, bb) = base;
    let ir = r.ceil() as i32;
    let r2 = r * r;
    for dy in -ir..=ir {
        for dx in -ir..=ir {
            let dist2 = (dx * dx + dy * dy) as f32;
            if dist2 > r2 {
                continue;
            }
            let px = (cx as i32 + dx) as u32;
            let py = (cy as i32 + dy) as u32;
            if px >= w || py >= buf.len() as u32 / w {
                continue;
            }
            let idx = (py * w + px) as usize;
            if idx >= buf.len() {
                continue;
            }

            let dist = dist2.sqrt();
            let frac = 1.0 - (dist / r);
            let alpha = (255.0 * intensity * frac * frac) as u8;
            buf[idx] = blend_over(buf[idx], rr, gg, bb, alpha);
        }
    }
}
