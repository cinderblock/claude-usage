//! Renders splat previews to PNGs so the design can be eyeballed before baking
//! it into the tray + app icon. `cargo run --bin preview_icons -- <out_dir>`.

use claude_usage_lib::splat;

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    std::fs::create_dir_all(&out).ok();

    // Status colors (grey / green / yellow / red).
    let colors = [
        ("grey", (110u8, 118u8, 129u8)),
        ("green", (46, 160, 67)),
        ("yellow", (223, 164, 12)),
        ("red", (210, 55, 43)),
    ];

    // Tray splats at a viewable size (transparent bg).
    for (name, c) in colors {
        let img = splat::tray_splat(128, c);
        let p = format!("{out}/tray_{name}.png");
        img.save(&p).unwrap();
        println!("wrote {p}");
    }

    // A contact sheet: the four tray splats on a charcoal strip.
    let sheet_w = 128 * 4;
    let sheet_h = 128;
    let mut sheet = image::RgbaImage::from_pixel(sheet_w, sheet_h, image::Rgba([28, 31, 36, 255]));
    for (i, (_, c)) in colors.iter().enumerate() {
        let s = splat::tray_splat(128, *c);
        image::imageops::overlay(&mut sheet, &s, (i as u32 * 128) as i64, 0);
    }
    sheet.save(format!("{out}/tray_row.png")).unwrap();
    println!("wrote {out}/tray_row.png");

    // App icon candidates: green splat on charcoal, and a grey/neutral variant.
    splat::app_icon(256, (46, 160, 67), (28, 31, 36))
        .save(format!("{out}/appicon_green_charcoal.png"))
        .unwrap();
    splat::app_icon(256, (223, 164, 12), (28, 31, 36))
        .save(format!("{out}/appicon_amber_charcoal.png"))
        .unwrap();
    splat::app_icon(256, (240, 246, 250), (28, 31, 36))
        .save(format!("{out}/appicon_white_charcoal.png"))
        .unwrap();
    println!("wrote app icon candidates");

    // 1024px master for `tauri icon` (chosen: green splat on charcoal).
    splat::app_icon(1024, (46, 160, 67), (28, 31, 36))
        .save(format!("{out}/master.png"))
        .unwrap();
    println!("wrote {out}/master.png");
}
