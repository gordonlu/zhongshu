#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrbState {
    Idle,
    Listening,
    Thinking { progress: f32 },
    Executing { pulse: f32 },
    Done { success: bool },
}

pub fn draw_orb(pixels: &mut [u32], w: u32, h: u32, state: OrbState, t: f64) {
    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;
    let r = (w.min(h) as f32 / 2.0) - 3.0;

    match state {
        OrbState::Idle => draw_idle(pixels, w, cx, cy, r),
        OrbState::Listening => draw_listening(pixels, w, cx, cy, r, t),
        OrbState::Thinking { progress } => draw_thinking(pixels, w, cx, cy, r, progress),
        OrbState::Executing { pulse } => draw_executing(pixels, w, cx, cy, r, pulse),
        OrbState::Done { success } => draw_done(pixels, w, cx, cy, r, success),
    }
}

fn draw_idle(buf: &mut [u32], w: u32, cx: f32, cy: f32, r: f32) {
    let red = pack(200, 40, 40, 255);
    fill_rect(buf, pack(24, 24, 28, 255));
    fill_circle_solid(buf, w, cx, cy, r, red);
}
fn draw_listening(buf: &mut [u32], w: u32, cx: f32, cy: f32, r: f32, t: f64) {
    let _ = t; draw_idle(buf, w, cx, cy, r);
}
fn draw_thinking(buf: &mut [u32], w: u32, cx: f32, cy: f32, r: f32, progress: f32) {
    let _ = progress; draw_idle(buf, w, cx, cy, r);
}
fn draw_executing(buf: &mut [u32], w: u32, cx: f32, cy: f32, r: f32, pulse: f32) {
    let _ = pulse; draw_idle(buf, w, cx, cy, r);
}
fn draw_done(buf: &mut [u32], w: u32, cx: f32, cy: f32, r: f32, success: bool) {
    let _ = success;
    fill_rect(buf, pack(24, 24, 28, 255));
    let green = pack(40, 200, 60, 255);
    fill_circle_solid(buf, w, cx, cy, r, green);
}

fn pack(r: u8, g: u8, b: u8, a: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

fn pack_rgb(r: u8, g: u8, b: u8) -> u32 {
    ((255u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

fn unpack(buf: u32) -> (u8, u8, u8, u8) {
    let a = ((buf >> 24) & 0xff) as u8;
    let r = ((buf >> 16) & 0xff) as u8;
    let g = ((buf >> 8) & 0xff) as u8;
    let b = (buf & 0xff) as u8;
    (r, g, b, a)
}

fn blend_over(dst: u32, r: u8, g: u8, b: u8, a: u8) -> u32 {
    if a == 0 { return dst; }
    if a == 255 { return pack_rgb(r, g, b); }

    let (dr, dg, db, da) = unpack(dst);
    let sa = a as f32 / 255.0;
    let da_f = da as f32 / 255.0;

    let out_a = sa + da_f * (1.0 - sa);
    if out_a < 0.001 { return dst; }

    let out_r = ((r as f32 * sa + dr as f32 * da_f * (1.0 - sa)) / out_a) as u8;
    let out_g = ((g as f32 * sa + dg as f32 * da_f * (1.0 - sa)) / out_a) as u8;
    let out_b = ((b as f32 * sa + db as f32 * da_f * (1.0 - sa)) / out_a) as u8;
    let out_a_u8 = (out_a * 255.0) as u8;
    pack(out_r, out_g, out_b, out_a_u8)
}

fn fill_circle_solid(buf: &mut [u32], w: u32, cx: f32, cy: f32, r: f32, color: u32) {
    let ir = r.ceil() as i32;
    let r2 = r * r;
    for dy in -ir..=ir {
        for dx in -ir..=ir {
            if (dx * dx + dy * dy) as f32 > r2 { continue; }
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
            if dist2 > r2 { continue; }
            let px = (cx as i32 + dx) as u32;
            let py = (cy as i32 + dy) as u32;
            if px >= w || py >= buf.len() as u32 / w { continue; }
            let idx = (py * w + px) as usize;
            if idx >= buf.len() { continue; }

            let dist = dist2.sqrt();
            let frac = 1.0 - (dist / r);
            let alpha = (255.0 * intensity * frac * frac) as u8;
            buf[idx] = blend_over(buf[idx], rr, gg, bb, alpha);
        }
    }
}

fn fill_rect(buf: &mut [u32], color: u32) {
    buf.fill(color);
}
