//! Fonts, colours and spacing.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, FontData, FontFamily, FontId, IconData, TextStyle, Visuals,
    epaint::text::{FontInsert, FontPriority, InsertFontFamily},
};

/// Korean-capable system fonts, tried in order. (font file, face index)
const KOREAN_FONTS: &[(&str, u32)] = &[
    // Windows
    ("C:\\Windows\\Fonts\\malgun.ttf", 0),
    ("C:\\Windows\\Fonts\\gulim.ttc", 0),
    // macOS
    ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0),
    ("/System/Library/Fonts/Supplemental/AppleGothic.ttf", 0),
    // Linux
    ("/usr/share/fonts/truetype/nanum/NanumGothic.ttf", 0),
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 1),
    ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 1),
    (
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        1,
    ),
    ("/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc", 0),
];

pub fn install_fonts(ctx: &egui::Context) {
    for (path, index) in KOREAN_FONTS {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let mut data = FontData::from_owned(bytes);
        data.index = *index;
        ctx.add_font(FontInsert {
            name: "korean".into(),
            data,
            families: vec![
                InsertFontFamily {
                    family: FontFamily::Proportional,
                    priority: FontPriority::Highest,
                },
                InsertFontFamily {
                    family: FontFamily::Monospace,
                    priority: FontPriority::Lowest,
                },
            ],
        });
        return;
    }
}

pub fn apply_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Heading, FontId::proportional(19.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.slider_width = 150.0;
        let dark = style.visuals.dark_mode;
        style.visuals.selection.bg_fill = accent(dark);
        style.visuals.hyperlink_color = accent(dark);
        style.visuals.widgets.hovered.corner_radius = 6.into();
        style.visuals.widgets.inactive.corner_radius = 6.into();
        style.visuals.widgets.active.corner_radius = 6.into();
    });
}

/// Fill of primary buttons (white text on it has ≥ 4.5:1 contrast).
pub const PRIMARY: Color32 = Color32::from_rgb(37, 99, 235);

/// Fill of destructive buttons (white text on it has ≥ 4.5:1 contrast).
pub const DANGER_FILL: Color32 = Color32::from_rgb(185, 28, 28);

pub fn accent(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(96, 145, 245)
    } else {
        Color32::from_rgb(37, 99, 235)
    }
}

pub fn success(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(74, 196, 120)
    } else {
        Color32::from_rgb(21, 128, 61)
    }
}

pub fn warning(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(240, 180, 80)
    } else {
        Color32::from_rgb(180, 83, 9)
    }
}

pub fn danger(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(248, 113, 113)
    } else {
        Color32::from_rgb(200, 30, 30)
    }
}

pub fn muted(v: &Visuals) -> Color32 {
    v.weak_text_color()
}

/// Soft background tint of a colour, for chips and banners.
pub fn tint(c: Color32, dark: bool) -> Color32 {
    let a = if dark { 40 } else { 28 };
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// Window icon: a page with crop marks, drawn in code.
pub fn app_icon() -> Arc<IconData> {
    const S: usize = 64;
    let mut rgba = vec![0u8; S * S * 4];
    let mut put = |x: usize, y: usize, c: [u8; 4]| {
        let i = (y * S + x) * 4;
        rgba[i..i + 4].copy_from_slice(&c);
    };
    let blue = [37, 99, 235, 255];
    let white = [255, 255, 255, 255];
    let ink = [30, 41, 59, 255];
    for y in 0..S {
        for x in 0..S {
            // rounded blue tile
            let (dx, dy) = (x.min(S - 1 - x), y.min(S - 1 - y));
            let corner = dx < 10 && dy < 10 && (10 - dx).pow(2) + (10 - dy).pow(2) > 100;
            if !corner {
                put(x, y, blue);
            }
        }
    }
    for y in 14..50 {
        for x in 18..46 {
            put(x, y, white); // page
        }
    }
    for y in 22..42 {
        for x in 24..40 {
            if (x == 24 || x == 39 || y == 22 || y == 41) || (x + y) % 9 == 0 {
                put(x, y, ink); // drawing
            }
        }
    }
    for i in 0..10 {
        for t in 0..3 {
            // crop corners
            put(8 + i, 8 + t, white);
            put(8 + t, 8 + i, white);
            put(S - 9 - i, S - 9 - t, white);
            put(S - 9 - t, S - 9 - i, white);
        }
    }
    Arc::new(IconData {
        rgba,
        width: S as u32,
        height: S as u32,
    })
}
