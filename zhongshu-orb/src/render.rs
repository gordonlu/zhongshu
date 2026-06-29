/// Procedural orb renderer for the resident Zhongshu indicator.
///
/// The renderer writes premultiplied 0xAARRGGBB pixels into a caller-owned
/// buffer. It intentionally avoids image assets and GPU dependencies so the
/// always-on Windows orb stays lightweight.

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum OrbMode {
    Idle,
    Listening,
    Thinking,
    Executing,
    WaitingApproval,
    DoneSuccess,
    DoneFailure,
    Offline,
    Processing,
}

#[derive(Debug, Clone, Copy)]
struct ModeProfile {
    period: f64,
    breath_amp: f64,
    shell_scale: f64,
    glow_scale: f64,
    turbulence: f64,
    flow_speed: f64,
    ribbon_alpha: f64,
    highlight: f64,
}

#[derive(Debug, Clone, Copy)]
struct ModePalette {
    deep: (u8, u8, u8),
    shadow: (u8, u8, u8),
    primary: (u8, u8, u8),
    secondary: (u8, u8, u8),
    tertiary: (u8, u8, u8),
    rim: (u8, u8, u8),
}

/// Draw an orb into `buf` (row-major 0xAARRGGBB pixels, size ww x hh).
#[allow(dead_code)]
pub fn draw_orb(buf: &mut [u32], ww: u32, hh: u32, cr: u8, cg: u8, cb: u8, t: f64, mode: OrbMode) {
    let pixel_count = (ww as usize).saturating_mul(hh as usize);
    if pixel_count == 0 || buf.len() < pixel_count {
        return;
    }
    buf[..pixel_count].fill(0);

    let small = ww.min(hh) < 56;
    let profile = mode_profile(mode, small);
    let palette = palette(mode, (cr, cg, cb));

    let (cx, cy) = (ww as f64 / 2.0, hh as f64 / 2.0);
    let max_r = (ww.min(hh) as f64 / 2.0).max(1.0);
    let base_r = max_r * profile.shell_scale;
    let breath = (t * std::f64::consts::TAU / profile.period).sin() * profile.breath_amp;
    let twist = t * profile.flow_speed * 0.22;
    let twist_sin = twist.sin();
    let twist_cos = twist.cos();

    for y in 0..hh {
        for x in 0..ww {
            let dx = x as f64 + 0.5 - cx;
            let dy = y as f64 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let angle = dy.atan2(dx);
            let ripple = angular_noise(angle, t, profile.flow_speed) * profile.turbulence;
            let radius = base_r * (1.0 + breath + ripple);
            let r = dist / radius.max(1.0);
            if r > profile.glow_scale {
                continue;
            }

            let u = dx / radius.max(1.0);
            let v = dy / radius.max(1.0);
            let sphere = (1.0 - smoothstep(0.99, 1.035, r)).max(0.0);
            let glow = (1.0 - smoothstep(1.0, profile.glow_scale, r)).max(0.0);
            let z = (1.0 - (u * u + v * v)).max(0.0).sqrt();
            let rim = smoothstep(0.82, 1.0, r) * sphere;
            let outer_rim =
                (1.0 - smoothstep(0.985, 1.025, r)).max(0.0) * smoothstep(0.91, 0.99, r) * sphere;

            let light = (u * -0.34 + v * -0.46 + z * 0.94).clamp(0.0, 1.0);
            let mut color = mix_rgb(palette.shadow, palette.deep, light * 0.62 + 0.12);
            color = mix_rgb(color, palette.primary, sphere * (0.1 + light * 0.18));

            if sphere > 0.0 {
                let ru = u * twist_cos - v * twist_sin;
                let rv = u * twist_sin + v * twist_cos;
                let ribbon = ribbon_energy(ru, rv, r, t, profile);
                let ribbon_core = ribbon_core_energy(ru, rv, r, t, profile);
                let reverse = ribbon_energy(-ru * 0.96, rv * 1.05, r, t + 0.41, profile) * 0.62;
                let reverse_core =
                    ribbon_core_energy(-ru * 0.96, rv * 1.05, r, t + 0.41, profile) * 0.52;
                let curl = internal_curl(angle, r, t, profile) * (1.0 - smoothstep(0.86, 1.0, r));

                let mut ribbon_color = mix_rgb(
                    palette.primary,
                    palette.secondary,
                    smoothstep(-0.58, 0.62, ru),
                );
                ribbon_color = mix_rgb(
                    ribbon_color,
                    palette.tertiary,
                    (rv + 0.64).clamp(0.0, 1.0) * 0.34,
                );
                color = mix_rgb(color, palette.tertiary, reverse * 0.3);
                color = mix_rgb(color, ribbon_color, ribbon * profile.ribbon_alpha);
                color = mix_rgb(
                    color,
                    (255, 255, 255),
                    ribbon_core * profile.ribbon_alpha * 0.34,
                );
                color = mix_rgb(
                    color,
                    palette.secondary,
                    reverse_core * profile.ribbon_alpha * 0.22,
                );
                color = mix_rgb(
                    color,
                    palette.rim,
                    (ribbon_core * 0.28 + reverse_core * 0.18 + curl * 0.2).clamp(0.0, 0.5),
                );

                let left_flash = elliptical_highlight(u, v, -0.36, -0.34, 0.34, 0.2);
                let upper_sheen = ribbon_sheen(u, v, t, profile);
                color = mix_rgb(
                    color,
                    palette.rim,
                    (left_flash * 0.42 + upper_sheen * 0.24) * profile.highlight,
                );
            }

            color = mix_rgb(color, palette.rim, rim * 0.16 + outer_rim * 0.26);

            let mut alpha = glow * 0.15 + sphere * 0.8 + outer_rim * 0.06;
            if small {
                alpha = alpha.max(sphere * 0.9);
            }

            if alpha <= 0.002 {
                continue;
            }

            buf[(y * ww + x) as usize] = premul_rgba(color, alpha.clamp(0.0, 1.0));
        }
    }
}

fn mode_profile(mode: OrbMode, small: bool) -> ModeProfile {
    let mut profile = match mode {
        OrbMode::Idle => ModeProfile {
            period: 4.2,
            breath_amp: 0.026,
            shell_scale: 0.62,
            glow_scale: 1.58,
            turbulence: 0.012,
            flow_speed: 0.54,
            ribbon_alpha: 0.7,
            highlight: 0.72,
        },
        OrbMode::Listening => ModeProfile {
            period: 1.9,
            breath_amp: 0.055,
            shell_scale: 0.63,
            glow_scale: 1.72,
            turbulence: 0.026,
            flow_speed: 1.46,
            ribbon_alpha: 0.92,
            highlight: 0.88,
        },
        OrbMode::Thinking => ModeProfile {
            period: 1.65,
            breath_amp: 0.05,
            shell_scale: 0.63,
            glow_scale: 1.72,
            turbulence: 0.038,
            flow_speed: 1.9,
            ribbon_alpha: 0.94,
            highlight: 0.86,
        },
        OrbMode::Executing => ModeProfile {
            period: 0.92,
            breath_amp: 0.068,
            shell_scale: 0.64,
            glow_scale: 1.78,
            turbulence: 0.05,
            flow_speed: 2.52,
            ribbon_alpha: 0.98,
            highlight: 0.82,
        },
        OrbMode::WaitingApproval => ModeProfile {
            period: 1.35,
            breath_amp: 0.05,
            shell_scale: 0.63,
            glow_scale: 1.72,
            turbulence: 0.03,
            flow_speed: 1.34,
            ribbon_alpha: 0.92,
            highlight: 0.86,
        },
        OrbMode::DoneSuccess => ModeProfile {
            period: 1.7,
            breath_amp: 0.028,
            shell_scale: 0.63,
            glow_scale: 1.7,
            turbulence: 0.018,
            flow_speed: 0.92,
            ribbon_alpha: 0.88,
            highlight: 0.86,
        },
        OrbMode::DoneFailure => ModeProfile {
            period: 1.2,
            breath_amp: 0.032,
            shell_scale: 0.63,
            glow_scale: 1.7,
            turbulence: 0.022,
            flow_speed: 0.86,
            ribbon_alpha: 0.82,
            highlight: 0.78,
        },
        OrbMode::Offline => ModeProfile {
            period: 4.8,
            breath_amp: 0.012,
            shell_scale: 0.62,
            glow_scale: 1.42,
            turbulence: 0.006,
            flow_speed: 0.28,
            ribbon_alpha: 0.34,
            highlight: 0.54,
        },
        OrbMode::Processing => ModeProfile {
            period: 1.15,
            breath_amp: 0.04,
            shell_scale: 0.62,
            glow_scale: 1.58,
            turbulence: 0.02,
            flow_speed: 1.28,
            ribbon_alpha: 0.38,
            highlight: 0.6,
        },
    };

    if small {
        profile.turbulence *= 0.35;
        profile.glow_scale = profile.glow_scale.min(1.45);
        profile.ribbon_alpha *= 0.72;
        profile.highlight *= 0.72;
    }
    profile
}

fn palette(mode: OrbMode, base: (u8, u8, u8)) -> ModePalette {
    let primary_blue = mix_rgb((57, 100, 254), base, 0.35);
    match mode {
        OrbMode::Idle => ModePalette {
            deep: (18, 35, 70),
            shadow: (4, 10, 22),
            primary: primary_blue,
            secondary: (237, 95, 224),
            tertiary: (96, 79, 240),
            rim: (218, 237, 255),
        },
        OrbMode::Listening => ModePalette {
            deep: (26, 46, 108),
            shadow: (6, 12, 28),
            primary: (255, 99, 219),
            secondary: (38, 207, 255),
            tertiary: (144, 110, 255),
            rim: (228, 240, 255),
        },
        OrbMode::Thinking => ModePalette {
            deep: (12, 39, 101),
            shadow: (3, 9, 28),
            primary: primary_blue,
            secondary: (32, 221, 244),
            tertiary: (172, 92, 255),
            rim: (224, 241, 255),
        },
        OrbMode::Executing => ModePalette {
            deep: (5, 61, 91),
            shadow: (3, 16, 28),
            primary: (31, 205, 244),
            secondary: (59, 236, 172),
            tertiary: (24, 126, 255),
            rim: (211, 252, 255),
        },
        OrbMode::WaitingApproval => ModePalette {
            deep: (52, 35, 68),
            shadow: (11, 9, 20),
            primary: (255, 92, 210),
            secondary: (255, 186, 30),
            tertiary: (132, 75, 255),
            rim: (255, 229, 246),
        },
        OrbMode::DoneSuccess => ModePalette {
            deep: (5, 71, 55),
            shadow: (3, 18, 18),
            primary: (34, 224, 165),
            secondary: (39, 191, 122),
            tertiary: (72, 240, 213),
            rim: (217, 255, 240),
        },
        OrbMode::DoneFailure => ModePalette {
            deep: (95, 24, 36),
            shadow: (24, 5, 10),
            primary: (255, 72, 98),
            secondary: (255, 127, 89),
            tertiary: (148, 45, 98),
            rim: (255, 222, 228),
        },
        OrbMode::Offline => ModePalette {
            deep: (37, 51, 68),
            shadow: (8, 13, 20),
            primary: (109, 133, 155),
            secondary: (172, 191, 210),
            tertiary: (56, 75, 94),
            rim: (216, 230, 244),
        },
        OrbMode::Processing => ModePalette {
            deep: (34, 48, 65),
            shadow: (7, 12, 19),
            primary: (109, 129, 154),
            secondary: (178, 194, 216),
            tertiary: (55, 72, 92),
            rim: (226, 237, 248),
        },
    }
}

fn ribbon_energy(u: f64, v: f64, r: f64, t: f64, profile: ModeProfile) -> f64 {
    let phase = t * profile.flow_speed * 0.58;
    let curve = 0.27 * (u * std::f64::consts::PI * 1.16 + phase).sin() - 0.06 * u;
    let width = 0.17 - 0.042 * r.clamp(0.0, 1.0);
    let band = 1.0 - smoothstep(width * 0.42, width, (v - curve).abs());
    let taper = (1.0 - smoothstep(0.68, 1.02, u.abs())).max(0.0);
    let sphere_clip = 1.0 - smoothstep(0.91, 1.02, r);
    let bright_core = 1.0 - smoothstep(0.0, width * 0.36, (v - curve).abs());
    (band * taper * sphere_clip + bright_core * taper * 0.62).clamp(0.0, 1.0)
}

fn ribbon_core_energy(u: f64, v: f64, r: f64, t: f64, profile: ModeProfile) -> f64 {
    let phase = t * profile.flow_speed * 0.58;
    let curve = 0.27 * (u * std::f64::consts::PI * 1.16 + phase).sin() - 0.06 * u;
    let width = 0.062 - 0.012 * r.clamp(0.0, 1.0);
    let band = 1.0 - smoothstep(width * 0.36, width, (v - curve).abs());
    let taper = 1.0 - smoothstep(0.56, 0.92, u.abs());
    let center_boost = 1.0 - smoothstep(0.0, 0.72, (u * u + v * v).sqrt());
    (band * taper.clamp(0.0, 1.0) * (0.62 + center_boost * 0.38)).clamp(0.0, 1.0)
}

fn ribbon_sheen(u: f64, v: f64, t: f64, profile: ModeProfile) -> f64 {
    let curve = -0.34 + 0.12 * (u * 3.5 + t * profile.flow_speed * 0.42).sin();
    let band = 1.0 - smoothstep(0.02, 0.18, (v - curve).abs());
    let taper = 1.0 - smoothstep(0.26, 0.88, (u + 0.22).abs());
    (band * taper).clamp(0.0, 1.0)
}

fn internal_curl(angle: f64, r: f64, t: f64, profile: ModeProfile) -> f64 {
    let wave = 0.5 + 0.5 * (angle * 3.1 + r * 7.6 - t * profile.flow_speed * 0.86).sin();
    let fine = 0.5 + 0.5 * (angle * -5.7 + r * 9.2 + t * profile.flow_speed * 0.5).sin();
    wave.powf(4.2) * fine.powf(1.5) * smoothstep(0.18, 0.92, r)
}

fn elliptical_highlight(u: f64, v: f64, cx: f64, cy: f64, rx: f64, ry: f64) -> f64 {
    let dx = (u - cx) / rx;
    let dy = (v - cy) / ry;
    1.0 - smoothstep(0.0, 1.0, (dx * dx + dy * dy).sqrt())
}

fn angular_noise(angle: f64, t: f64, speed: f64) -> f64 {
    let a = (angle * 3.0 + t * speed).sin();
    let b = (angle * -5.0 + t * speed * 0.73).sin();
    let c = (angle * 7.0 - t * speed * 1.17).cos();
    (a * 0.55 + b * 0.32 + c * 0.18) / 1.05
}

fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    if (edge1 - edge0).abs() < f64::EPSILON {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let x = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn mix_rgb(a: (u8, u8, u8), b: (u8, u8, u8), amount: f64) -> (u8, u8, u8) {
    let amount = amount.clamp(0.0, 1.0);
    (
        (a.0 as f64 + (b.0 as f64 - a.0 as f64) * amount) as u8,
        (a.1 as f64 + (b.1 as f64 - a.1 as f64) * amount) as u8,
        (a.2 as f64 + (b.2 as f64 - a.2 as f64) * amount) as u8,
    )
}

fn premul_rgba(color: (u8, u8, u8), alpha: f64) -> u32 {
    let a = (alpha * 255.0).round().clamp(0.0, 255.0) as u32;
    let af = a as f64 / 255.0;
    let r = (color.0 as f64 * af).round().clamp(0.0, 255.0) as u32;
    let g = (color.1 as f64 * af).round().clamp(0.0, 255.0) as u32;
    let b = (color.2 as f64 * af).round().clamp(0.0, 255.0) as u32;
    (a << 24) | (r << 16) | (g << 8) | b
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
            let scale = if a == 0 { 0.0 } else { 255.0 / a as f64 };
            data.push((r as f64 * scale).min(255.0) as u8);
            data.push((g as f64 * scale).min(255.0) as u8);
            data.push((b as f64 * scale).min(255.0) as u8);
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

        for (mode_name, mode) in all_modes() {
            for (phase_name, t) in &[("t0", 0.0), ("t_peak", 1.0), ("t_dip", 0.5)] {
                buf.fill(0);
                draw_orb(&mut buf, ww, hh, 57, 100, 254, *t, mode);
                let path = base_dir.join(format!("orb_{mode_name}_{phase_name}.ppm"));
                write_ppm(path.to_str().unwrap(), &buf, ww, hh);
            }
        }

        buf.fill(0);
        draw_orb(&mut buf, ww, hh, 57, 100, 254, 0.0, OrbMode::Idle);
        let centre = buf[hh as usize / 2 * ww as usize + ww as usize / 2];
        assert_ne!(centre & 0x00FFFFFF, 0, "centre pixel should have colour");

        let centre_alpha = (centre >> 24) as u32;
        let corner_alpha = (buf[0] >> 24) as u32;
        assert!(
            centre_alpha > corner_alpha * 8,
            "centre alpha ({centre_alpha}) should be much higher than corner ({corner_alpha})"
        );

        eprintln!("Orb PPM frames written to {}/", base_dir.display());
    }

    #[test]
    fn orb_animation_changes_frame_pixels() {
        let (ww, hh) = (96u32, 96u32);
        let mut a = vec![0u32; (ww * hh) as usize];
        let mut b = vec![0u32; (ww * hh) as usize];
        draw_orb(&mut a, ww, hh, 57, 100, 254, 0.0, OrbMode::Executing);
        draw_orb(&mut b, ww, hh, 57, 100, 254, 0.33, OrbMode::Executing);

        let changed = a
            .iter()
            .zip(&b)
            .filter(|(left, right)| left != right)
            .count();
        assert!(
            changed > (ww * hh / 8) as usize,
            "active orb should animate a meaningful number of pixels"
        );
    }

    #[test]
    fn orb_clears_stale_pixels() {
        let (ww, hh) = (64u32, 64u32);
        let mut buf = vec![0xFFFFFFFFu32; (ww * hh) as usize];
        draw_orb(&mut buf, ww, hh, 57, 100, 254, 0.0, OrbMode::Idle);
        assert_eq!(buf[0], 0, "transparent corner should be cleared each frame");
    }

    #[test]
    fn mode_palettes_are_visibly_distinct() {
        let (ww, hh) = (96u32, 96u32);
        let centre = |mode| {
            let mut buf = vec![0u32; (ww * hh) as usize];
            draw_orb(&mut buf, ww, hh, 57, 100, 254, 0.2, mode);
            buf[hh as usize / 2 * ww as usize + ww as usize / 2] & 0x00FFFFFF
        };
        assert_ne!(centre(OrbMode::Idle), centre(OrbMode::Thinking));
        assert_ne!(centre(OrbMode::Thinking), centre(OrbMode::Executing));
        assert_ne!(centre(OrbMode::DoneSuccess), centre(OrbMode::DoneFailure));
        assert_ne!(centre(OrbMode::Offline), centre(OrbMode::Listening));
    }

    fn all_modes() -> [(&'static str, OrbMode); 9] {
        [
            ("idle", OrbMode::Idle),
            ("listening", OrbMode::Listening),
            ("thinking", OrbMode::Thinking),
            ("executing", OrbMode::Executing),
            ("waiting_approval", OrbMode::WaitingApproval),
            ("done_success", OrbMode::DoneSuccess),
            ("done_failure", OrbMode::DoneFailure),
            ("offline", OrbMode::Offline),
            ("processing", OrbMode::Processing),
        ]
    }
}
