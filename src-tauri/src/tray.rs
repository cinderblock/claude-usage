//! System-tray icon: a Claude-style "splat" tinted by status
//! (grey idle / green ok / yellow warning / red critical). The live percent
//! lives in the tooltip, right-click menu, and popup — not on the icon.

use crate::splat;

const SIZE: u32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Critical,
    Unknown,
}

impl Status {
    pub fn rgb(self) -> (u8, u8, u8) {
        match self {
            Status::Ok => (46, 160, 67),        // green
            Status::Warn => (223, 164, 12),     // yellow/amber
            Status::Critical => (210, 55, 43),  // red
            Status::Unknown => (110, 118, 129), // grey
        }
    }
}

/// Render the tray icon (transparent-background splat tinted by status).
pub fn render_icon(status: Status) -> tauri::image::Image<'static> {
    let img = splat::tray_splat(SIZE, status.rgb());
    tauri::image::Image::new_owned(img.into_raw(), SIZE, SIZE)
}
