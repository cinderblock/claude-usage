//! Renders the system-tray icon: a color-coded badge showing the live percent.
//! Uses a system font if one is found; otherwise falls back to a filled ring.

use ab_glyph::{Font, FontVec, PxScale};
use image::{Rgba, RgbaImage};
use std::sync::OnceLock;

const SIZE: u32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Critical,
    Unknown,
}

impl Status {
    fn rgb(self) -> (u8, u8, u8) {
        match self {
            Status::Ok => (46, 160, 67),       // green
            Status::Warn => (219, 154, 4),      // amber
            Status::Critical => (210, 55, 43),  // red
            Status::Unknown => (110, 118, 129), // gray
        }
    }
}

fn font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        let candidates = [
            r"C:\Windows\Fonts\segoeuib.ttf",
            r"C:\Windows\Fonts\arialbd.ttf",
            r"C:\Windows\Fonts\seguisb.ttf",
            r"C:\Windows\Fonts\segoeui.ttf",
            r"C:\Windows\Fonts\arial.ttf",
        ];
        for path in candidates {
            if let Ok(bytes) = std::fs::read(path) {
                if let Ok(f) = FontVec::try_from_vec(bytes) {
                    return Some(f);
                }
            }
        }
        None
    })
    .as_ref()
}

fn blend(dst: &mut Rgba<u8>, src: (u8, u8, u8), a: f32) {
    let a = a.clamp(0.0, 1.0);
    let s = [src.0, src.1, src.2];
    for i in 0..3 {
        dst[i] = (s[i] as f32 * a + dst[i] as f32 * (1.0 - a)).round() as u8;
    }
    dst[3] = ((dst[3] as f32).max(a * 255.0)).round() as u8;
}

fn fill_disc(img: &mut RgbaImage, color: (u8, u8, u8)) {
    let c = (SIZE as f32 - 1.0) / 2.0;
    let r = SIZE as f32 / 2.0 - 0.5;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let d = (((x as f32 - c).powi(2)) + ((y as f32 - c).powi(2))).sqrt();
            let a = (r - d + 0.5).clamp(0.0, 1.0); // 1px anti-aliased edge
            if a > 0.0 {
                blend(img.get_pixel_mut(x, y), color, a);
            }
        }
    }
}

/// Ring fallback: draw an arc proportional to `percent` in white over the disc.
fn draw_ring(img: &mut RgbaImage, percent: f64) {
    let c = (SIZE as f32 - 1.0) / 2.0;
    let outer = SIZE as f32 / 2.0 - 2.0;
    let inner = outer - 5.0;
    let frac = (percent / 100.0).clamp(0.0, 1.0) as f32;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - c;
            let dy = y as f32 - c;
            let d = (dx * dx + dy * dy).sqrt();
            if d <= outer && d >= inner {
                // angle from top, clockwise, 0..1
                let mut ang = (dx).atan2(-dy); // 0 at top
                if ang < 0.0 {
                    ang += std::f32::consts::TAU;
                }
                let a = ang / std::f32::consts::TAU;
                if a <= frac {
                    blend(img.get_pixel_mut(x, y), (255, 255, 255), 1.0);
                }
            }
        }
    }
}

fn draw_text_centered(img: &mut RgbaImage, font: &FontVec, text: &str, color: (u8, u8, u8)) {
    // Pick a scale that fits the width for 1-3 chars.
    let px: f32 = match text.len() {
        1 => 22.0,
        2 => 20.0,
        _ => 15.0,
    };
    let scale = PxScale::from(px);
    let scaled = font.as_scaled(scale);
    use ab_glyph::ScaleFont;

    // Lay out glyphs, measuring total advance.
    let mut advances = Vec::new();
    let mut total_w = 0.0f32;
    for ch in text.chars() {
        let g = font.glyph_id(ch);
        let adv = scaled.h_advance(g);
        advances.push((g, adv));
        total_w += adv;
    }
    let ascent = scaled.ascent();
    let descent = scaled.descent();
    let text_h = ascent - descent;
    let mut pen_x = (SIZE as f32 - total_w) / 2.0;
    let baseline_y = (SIZE as f32 + text_h) / 2.0 - descent - text_h * 0.08;

    for (gid, adv) in advances {
        let glyph = gid.with_scale_and_position(scale, ab_glyph::point(pen_x, baseline_y));
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                let px_ = bounds.min.x as i32 + gx as i32;
                let py_ = bounds.min.y as i32 + gy as i32;
                if px_ >= 0 && py_ >= 0 && (px_ as u32) < SIZE && (py_ as u32) < SIZE {
                    blend(img.get_pixel_mut(px_ as u32, py_ as u32), color, cov);
                }
            });
        }
        pen_x += adv;
    }
}

/// Render the tray icon as a Tauri image (RGBA).
pub fn render_icon(percent: f64, status: Status) -> tauri::image::Image<'static> {
    let mut img = RgbaImage::new(SIZE, SIZE);
    fill_disc(&mut img, status.rgb());

    let label = format!("{}", percent.round().clamp(0.0, 100.0) as i32);
    match font() {
        Some(f) => draw_text_centered(&mut img, f, &label, (255, 255, 255)),
        None => draw_ring(&mut img, percent),
    }

    let raw = img.into_raw();
    tauri::image::Image::new_owned(raw, SIZE, SIZE)
}
