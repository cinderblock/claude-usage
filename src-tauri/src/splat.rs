//! A "splat" — a sunburst/spark that riffs on Claude's mark — rendered
//! procedurally so it can be tinted per status (grey/green/yellow/red) for the
//! tray and baked into the app icon. No external assets or SVG rasterizer.

use image::{Rgba, RgbaImage};

/// Spikes as (angle_degrees, length_fraction). Math angles (0 = right, 90 = up).
/// Slightly irregular lengths give it the organic, hand-thrown "splat" feel
/// rather than a rigid symmetric star.
const SPIKES: &[(f32, f32)] = &[
    (90.0, 1.00),
    (118.0, 0.70),
    (150.0, 0.90),
    (182.0, 0.68),
    (210.0, 0.96),
    (238.0, 0.71),
    (270.0, 0.86),
    (300.0, 0.68),
    (328.0, 0.92),
    (2.0, 0.71),
    (30.0, 0.97),
    (60.0, 0.73),
];

const SS: u32 = 4; // supersampling per axis (SS*SS samples/pixel)

/// Is a point (relative to center) inside the splat of outer radius `r`?
fn splat_hit(dx: f32, dy: f32, r: f32) -> bool {
    let disc = r * 0.17;
    if dx * dx + dy * dy <= disc * disc {
        return true;
    }
    let w0 = r * 0.155;
    for &(deg, lenf) in SPIKES {
        let a = deg.to_radians();
        let (ax, ay) = (a.cos(), -a.sin()); // screen y is down
        let s = dx * ax + dy * ay; // along the spike
        let len = r * lenf;
        if s <= 0.0 || s >= len {
            continue;
        }
        let d = dx * (-ay) + dy * ax; // perpendicular
        let frac = s / len;
        let hw = w0 * (1.0 - frac).powf(0.85);
        if d.abs() <= hw {
            return true;
        }
    }
    false
}

/// Anti-aliased coverage [0,1] of the splat at pixel (px,py).
fn splat_coverage(px: u32, py: u32, c: f32, r: f32) -> f32 {
    let mut hits = 0u32;
    for sy in 0..SS {
        for sx in 0..SS {
            let fx = px as f32 + (sx as f32 + 0.5) / SS as f32 - c;
            let fy = py as f32 + (sy as f32 + 0.5) / SS as f32 - c;
            if splat_hit(fx, fy, r) {
                hits += 1;
            }
        }
    }
    hits as f32 / (SS * SS) as f32
}

/// Rounded-square coverage [0,1] for the app-icon background.
fn rrect_coverage(px: u32, py: u32, size: f32, radius: f32) -> f32 {
    let mut hits = 0u32;
    let half = size / 2.0;
    for sy in 0..SS {
        for sx in 0..SS {
            let fx = px as f32 + (sx as f32 + 0.5) / SS as f32 - half;
            let fy = py as f32 + (sy as f32 + 0.5) / SS as f32 - half;
            let qx = fx.abs() - (half - radius);
            let qy = fy.abs() - (half - radius);
            let outside = qx.max(0.0).hypot(qy.max(0.0)) + qx.min(0.0).max(qy.min(0.0)) - radius;
            if outside <= 0.0 {
                hits += 1;
            }
        }
    }
    hits as f32 / (SS * SS) as f32
}

fn over(dst: &mut Rgba<u8>, color: (u8, u8, u8), a: f32) {
    let a = a.clamp(0.0, 1.0);
    let s = [color.0, color.1, color.2];
    let da = dst[3] as f32 / 255.0;
    let out_a = a + da * (1.0 - a);
    if out_a <= 0.0 {
        return;
    }
    for i in 0..3 {
        let sc = s[i] as f32;
        let dc = dst[i] as f32;
        dst[i] = ((sc * a + dc * da * (1.0 - a)) / out_a).round().clamp(0.0, 255.0) as u8;
    }
    dst[3] = (out_a * 255.0).round() as u8;
}

/// Transparent-background splat tinted `color`, for the tray.
pub fn tray_splat(size: u32, color: (u8, u8, u8)) -> RgbaImage {
    let mut img = RgbaImage::new(size, size);
    let c = (size as f32 - 1.0) / 2.0;
    let r = size as f32 * 0.47;
    for y in 0..size {
        for x in 0..size {
            let cov = splat_coverage(x, y, c, r);
            if cov > 0.0 {
                over(img.get_pixel_mut(x, y), color, cov);
            }
        }
    }
    img
}

/// App icon: splat over a rounded-square background.
pub fn app_icon(size: u32, splat: (u8, u8, u8), bg: (u8, u8, u8)) -> RgbaImage {
    let mut img = RgbaImage::new(size, size);
    let fsize = size as f32;
    let radius = fsize * 0.22;
    let c = (fsize - 1.0) / 2.0;
    let r = fsize * 0.40;
    for y in 0..size {
        for x in 0..size {
            let bgc = rrect_coverage(x, y, fsize, radius);
            if bgc > 0.0 {
                over(img.get_pixel_mut(x, y), bg, bgc);
            }
            let sc = splat_coverage(x, y, c, r);
            if sc > 0.0 {
                over(img.get_pixel_mut(x, y), splat, sc);
            }
        }
    }
    img
}
