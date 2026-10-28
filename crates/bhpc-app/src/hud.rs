//! What the window draws over the game picture: the FPS counter and short messages (a tiny built-in 5x7 bitmap font,
//! light text on a semi-transparent dark rounded box) and the controller cursor ([`Sprite`]s placed anywhere on the
//! game picture). Pictures are premultiplied RGBA, top row first, ready to be blended by the window (`vsurface`).

use bhpc_gles::Overlay;
use std::sync::Arc;

/// Glyph cell size in font pixels.
pub const GLYPH_W: u32 = 5;
pub const GLYPH_H: u32 = 7;
/// Text colour (premultiplied, opaque).
pub const TEXT: [u8; 4] = [245, 245, 245, 255];
/// Box colour: black at this alpha (premultiplied, so the colour channels stay 0).
pub const BOX_ALPHA: f32 = 0.85;

/// Rows of a glyph, the top row first, bit 4 = leftmost column. `None` for characters the font does not have
/// (they are drawn as blanks). Letters are upper case only (lower case is drawn the same): Latin with the Turkish
/// letters, and Cyrillic with the Kazakh ones. Cyrillic letters shaped like Latin ones share their glyphs.
pub fn glyph(c: char) -> Option<[u8; 7]> {
    let c = c.to_uppercase().next().unwrap_or(c);
    let c = match c {
        'А' => 'A',
        'В' => 'B',
        'Е' => 'E',
        'К' => 'K',
        'М' => 'M',
        'Н' => 'H',
        'О' => 'O',
        'Р' => 'P',
        'С' => 'C',
        'Т' => 'T',
        'Х' => 'X',
        'Ү' => 'Y',
        'І' => 'I',
        other => other,
    };
    Some(match c {
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        '.' => [0, 0, 0, 0, 0, 0b01100, 0b01100],
        ':' => [0, 0b01100, 0b01100, 0, 0b01100, 0b01100, 0],
        '/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        '(' => [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
        ')' => [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0, 0b00100],
        ',' => [0, 0, 0, 0, 0b01100, 0b00100, 0b01000],
        '\'' | '’' => [0b00100, 0b00100, 0b01000, 0, 0, 0, 0],
        // Turkish.
        'Ç' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10001, 0b01110, 0b00100],
        'Ğ' => [0b01110, 0, 0b01111, 0b10000, 0b10111, 0b10001, 0b01111],
        'İ' => [0b00100, 0, 0b01110, 0b00100, 0b00100, 0b00100, 0b01110],
        'Ö' => [0b01010, 0, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110],
        'Ş' => [0b01111, 0b10000, 0b01110, 0b00001, 0b11110, 0b00100, 0b01100],
        'Ü' => [0b01010, 0, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        // Cyrillic (Russian).
        'Б' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b11110],
        'Г' => [0b11111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000],
        'Д' => [0b00110, 0b01010, 0b01010, 0b01010, 0b01010, 0b11111, 0b10001],
        'Ё' => [0b01010, 0, 0b11111, 0b10000, 0b11110, 0b10000, 0b11111],
        'Ж' => [0b10101, 0b10101, 0b10101, 0b01110, 0b10101, 0b10101, 0b10101],
        'З' => [0b01110, 0b10001, 0b00001, 0b00110, 0b00001, 0b10001, 0b01110],
        'И' => [0b10001, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b10001],
        'Й' => [0b01110, 0, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001],
        'Л' => [0b00111, 0b01001, 0b01001, 0b01001, 0b01001, 0b01001, 0b10001],
        'П' => [0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001],
        'У' => [0b10001, 0b10001, 0b10001, 0b01111, 0b00001, 0b10001, 0b01110],
        'Ф' => [0b00100, 0b01110, 0b10101, 0b10101, 0b10101, 0b01110, 0b00100],
        'Ц' => [0b10010, 0b10010, 0b10010, 0b10010, 0b10010, 0b11111, 0b00001],
        'Ч' => [0b10001, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b00001],
        'Ш' => [0b10101, 0b10101, 0b10101, 0b10101, 0b10101, 0b10101, 0b11111],
        'Щ' => [0b10101, 0b10101, 0b10101, 0b10101, 0b10101, 0b11111, 0b00001],
        'Ъ' => [0b11000, 0b01000, 0b01000, 0b01110, 0b01001, 0b01001, 0b01110],
        'Ы' => [0b10001, 0b10001, 0b10001, 0b11101, 0b10011, 0b10011, 0b11101],
        'Ь' => [0b10000, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b11110],
        'Э' => [0b01110, 0b10001, 0b00001, 0b00111, 0b00001, 0b10001, 0b01110],
        'Ю' => [0b10010, 0b10101, 0b10101, 0b11101, 0b10101, 0b10101, 0b10010],
        'Я' => [0b01111, 0b10001, 0b10001, 0b01111, 0b00101, 0b01001, 0b10001],
        // Kazakh.
        'Ә' => [0b01110, 0b10001, 0b00001, 0b11111, 0b10001, 0b10001, 0b01110],
        'Ғ' => [0b01111, 0b01000, 0b01000, 0b11110, 0b01000, 0b01000, 0b01000],
        'Қ' => [0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10011, 0b00001],
        'Ң' => [0b10010, 0b10010, 0b10010, 0b11110, 0b10010, 0b10011, 0b00001],
        'Ө' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b01110],
        'Ұ' => [0b10001, 0b10001, 0b01010, 0b00100, 0b11111, 0b00100, 0b00100],
        'Һ' => [0b10000, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b10001],
        _ => return None,
    })
}

/// Language of the host's messages over the game (`bhpc-run --lang`, the launcher's language).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    En,
    Tr,
    Ru,
    Kk,
}

impl Lang {
    /// `en`, `tr`, `ru` or `kk`, with or without a region (`ru-RU`, `kk_KZ`); anything else is English.
    pub fn parse(s: &str) -> Lang {
        match s.split(['-', '_']).next().unwrap_or_default().to_ascii_lowercase().as_str() {
            "tr" => Lang::Tr,
            "ru" => Lang::Ru,
            "kk" => Lang::Kk,
            _ => Lang::En,
        }
    }
}

/// The host's messages: English (the text the code posts), Turkish, Russian, Kazakh. Upper case, in the
/// characters [`glyph`] draws.
const MESSAGES: [[&str; 4]; 7] = [
    ["GAME BUTTONS HIDDEN", "OYUN DÜĞMELERİ GİZLENDİ", "КНОПКИ ИГРЫ СКРЫТЫ", "ОЙЫН БАТЫРМАЛАРЫ ЖАСЫРЫЛДЫ"],
    ["GAME BUTTONS SHOWN", "OYUN DÜĞMELERİ GÖSTERİLDİ", "КНОПКИ ИГРЫ ПОКАЗАНЫ", "ОЙЫН БАТЫРМАЛАРЫ КӨРСЕТІЛДІ"],
    [
        "GAME BUTTONS CANNOT BE HIDDEN",
        "OYUN DÜĞMELERİ GİZLENEMİYOR",
        "КНОПКИ ИГРЫ НЕЛЬЗЯ СКРЫТЬ",
        "ОЙЫН БАТЫРМАЛАРЫН ЖАСЫРУ МҮМКІН ЕМЕС",
    ],
    ["COPIED TO CLIPBOARD", "PANOYA KOPYALANDI", "СКОПИРОВАНО В БУФЕР ОБМЕНА", "АЛМАСУ БУФЕРІНЕ КӨШІРІЛДІ"],
    [
        "ACHIEVEMENTS ARE NOT AVAILABLE ON PC",
        "BAŞARIMLAR PC’DE KULLANILAMAZ",
        "ДОСТИЖЕНИЯ НА ПК НЕДОСТУПНЫ",
        "ЖЕТІСТІКТЕР КОМПЬЮТЕРДЕ ҚОЛЖЕТІМСІЗ",
    ],
    [
        "IN-APP PURCHASES ARE NOT AVAILABLE ON PC",
        "UYGULAMA İÇİ SATIN ALMA PC’DE KULLANILAMAZ",
        "ПОКУПКИ В ИГРЕ НА ПК НЕДОСТУПНЫ",
        "ОЙЫН ІШІНДЕГІ САТЫП АЛУ КОМПЬЮТЕРДЕ ҚОЛЖЕТІМСІЗ",
    ],
    [
        "NO FILE DIALOG: INSTALL ZENITY OR KDIALOG",
        "DOSYA SEÇME PENCERESİ YOK: ZENITY YA DA KDIALOG KURUN",
        "НЕТ ОКНА ВЫБОРА ФАЙЛА: УСТАНОВИТЕ ZENITY ИЛИ KDIALOG",
        "ФАЙЛ ТАҢДАУ ТЕРЕЗЕСІ ЖОҚ: ZENITY НЕМЕСЕ KDIALOG ОРНАТЫҢЫЗ",
    ],
];

/// The message posted when a photo was saved, followed by its path, in each language (order of [`MESSAGES`]).
pub const PHOTO_SAVED: [&str; 4] = ["PHOTO SAVED: ", "FOTOĞRAF KAYDEDİLDİ: ", "ФОТО СОХРАНЕНО: ", "ФОТО САҚТАЛДЫ: "];

/// `text` (a host message in English) in `lang`; texts without a translation are shown as they are.
pub fn localise(lang: Lang, text: &str) -> String {
    let i = lang as usize;
    if let Some(m) = MESSAGES.iter().find(|m| m[0] == text) {
        return m[i].to_string();
    }
    match text.strip_prefix(PHOTO_SAVED[0]) {
        Some(path) => format!("{}{path}", PHOTO_SAVED[i]),
        None => text.to_string(),
    }
}

/// A premultiplied RGBA picture, top row first.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Picture {
    /// The pixel at (`x`, `y`).
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
}

/// Coverage (0..=1) of the rounded box at pixel (`x`, `y`) of a `w` x `h` box with corner radius `r`.
fn box_coverage(x: u32, y: u32, w: u32, h: u32, r: u32) -> f32 {
    let (px, py, r) = (x as f32 + 0.5, y as f32 + 0.5, r as f32);
    // Distance into a corner square, measured from that corner's circle centre.
    let cx = if px < r {
        r - px
    } else if px > w as f32 - r {
        px - (w as f32 - r)
    } else {
        0.0
    };
    let cy = if py < r {
        r - py
    } else if py > h as f32 - r {
        py - (h as f32 - r)
    } else {
        0.0
    };
    if cx == 0.0 || cy == 0.0 {
        return 1.0;
    }
    (r - (cx * cx + cy * cy).sqrt() + 0.5).clamp(0.0, 1.0)
}

/// `text` in the bitmap font, each font pixel `scale` x `scale` screen pixels, on the rounded box.
pub fn render_label(text: &str, scale: u32) -> Picture {
    let s = scale.max(1);
    let (pad_x, pad_y) = (3 * s, 3 * s);
    let n = text.chars().count() as u32;
    let text_w = if n == 0 { 0 } else { n * (GLYPH_W + 1) * s - s };
    let (w, h) = (text_w + 2 * pad_x, GLYPH_H * s + 2 * pad_y);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let a = (box_coverage(x, y, w, h, 3 * s) * BOX_ALPHA * 255.0).round() as u8;
            rgba[((y * w + x) * 4 + 3) as usize] = a;
        }
    }
    for (i, c) in text.chars().enumerate() {
        let Some(rows) = glyph(c) else { continue };
        let x0 = pad_x + i as u32 * (GLYPH_W + 1) * s;
        for (gy, bits) in rows.iter().enumerate() {
            for gx in 0..GLYPH_W {
                if bits & (1 << (GLYPH_W - 1 - gx)) == 0 {
                    continue;
                }
                for dy in 0..s {
                    for dx in 0..s {
                        let (x, y) = (x0 + gx * s + dx, pad_y + gy as u32 * s + dy);
                        let at = ((y * w + x) * 4) as usize;
                        rgba[at..at + 4].copy_from_slice(&TEXT);
                    }
                }
            }
        }
    }
    Picture { width: w, height: h, rgba }
}

/// The counter's text: the rate rounded to whole frames.
pub fn fps_label(fps: f32) -> String {
    format!("{} FPS", fps.max(0.0).round() as u32)
}

/// Font scale for a game picture `height` pixels tall: 2 up to 719 px, then one more per 360 px (3 at 1080p).
pub fn hud_scale(height: u32) -> u32 {
    (height / 360).clamp(2, 8)
}

/// The corner of the game picture the counter sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Corner {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    /// `tl`, `tr`, `bl` or `br` (the `--fps-position` values).
    pub fn parse(s: &str) -> Option<Corner> {
        Some(match s {
            "tl" => Corner::TopLeft,
            "tr" => Corner::TopRight,
            "bl" => Corner::BottomLeft,
            "br" => Corner::BottomRight,
            _ => return None,
        })
    }
}

/// The counter as the window draws it: the picture, its distance from the game picture's edges and its corner.
#[derive(Debug, Clone)]
pub struct Hud {
    pub overlay: Overlay,
    pub margin: u32,
    pub corner: Corner,
}

impl Hud {
    /// Where the counter goes inside the game picture `(x, y, w, h)` of a GL viewport (rows counted from the bottom):
    /// the lower-left pixel of the counter, `margin` pixels from the two edges of its corner.
    pub fn origin(&self, (x, y, w, h): (i32, i32, i32, i32)) -> (i32, i32) {
        let (ow, oh, m) = (self.overlay.width as i32, self.overlay.height as i32, self.margin as i32);
        let left = matches!(self.corner, Corner::TopLeft | Corner::BottomLeft);
        let top = matches!(self.corner, Corner::TopLeft | Corner::TopRight);
        (if left { x + m } else { x + w - m - ow }, if top { y + h - m - oh } else { y + m })
    }
}

/// The counter for `fps` on a game picture `height` pixels tall, in `corner`; `serial` must change with the pixels.
pub fn fps_hud(fps: f32, serial: u64, height: u32, corner: Corner) -> Hud {
    let s = hud_scale(height);
    let p = render_label(&fps_label(fps), s);
    Hud { overlay: Overlay { width: p.width, height: p.height, serial, rgba: Arc::new(p.rgba) }, margin: 4 * s, corner }
}

/// A picture placed anywhere on the game picture (the controller cursor, a message).
#[derive(Debug, Clone)]
pub struct Sprite {
    pub overlay: Overlay,
    /// Where its centre goes, as fractions of the game picture (0, 0 is the top-left corner).
    pub at: (f32, f32),
}

impl Sprite {
    /// The lower-left pixel of the sprite inside the game picture `(x, y, w, h)` of a GL viewport (rows counted from
    /// the bottom).
    pub fn origin(&self, (x, y, w, h): (i32, i32, i32, i32)) -> (i32, i32) {
        let (ow, oh) = (self.overlay.width as f32, self.overlay.height as f32);
        let cx = x as f32 + self.at.0 * w as f32;
        let cy = y as f32 + (1.0 - self.at.1) * h as f32;
        ((cx - ow / 2.0).round() as i32, (cy - oh / 2.0).round() as i32)
    }
}

/// Cursor colours: a light frame with a dark outline (premultiplied, opaque).
pub const CURSOR_LIGHT: [u8; 4] = [255, 236, 140, 255];
pub const CURSOR_DARK: [u8; 4] = [20, 20, 20, 255];

/// The controller cursor: the corners of a `size` x `size` pixel frame (one tile on screen) and a dot in the middle,
/// light with a dark outline so it shows on any background.
pub fn cursor_picture(size: u32) -> Picture {
    let s = size.clamp(12, 512);
    let t = (s / 16).max(2); // line thickness
    let arm = (s / 4).max(3 * t); // corner arm length
    let mut rgba = vec![0u8; (s * s * 4) as usize];
    let light = |x: u32, y: u32| {
        let (ex, ey) = (x.min(s - 1 - x), y.min(s - 1 - y)); // distance from the nearest edges
        let corner = ex >= 1 && ey >= 1 && ((ex <= t && ey <= arm) || (ey <= t && ex <= arm));
        let (cx, cy) = (x.abs_diff(s / 2), y.abs_diff(s / 2));
        corner || (cx < t && cy < t)
    };
    for y in 0..s {
        for x in 0..s {
            let px = if light(x, y) {
                CURSOR_LIGHT
            } else {
                // The outline: next to a light pixel.
                let near = (x.saturating_sub(1)..=(x + 1).min(s - 1))
                    .any(|nx| (y.saturating_sub(1)..=(y + 1).min(s - 1)).any(|ny| light(nx, ny)));
                if near {
                    CURSOR_DARK
                } else {
                    continue;
                }
            };
            let at = ((y * s + x) * 4) as usize;
            rgba[at..at + 4].copy_from_slice(&px);
        }
    }
    Picture { width: s, height: s, rgba }
}

/// Mic-on indicator colours (premultiplied, opaque): a red disc with a white microphone.
pub const MIC_RED: [u8; 4] = [214, 40, 40, 255];
pub const MIC_WHITE: [u8; 4] = [255, 255, 255, 255];

/// The host's mic-on indicator, `size` x `size` pixels: shown while the microphone records, drawn by the host over
/// everything (mods cannot hide or fake it).
pub fn mic_picture(size: u32) -> Picture {
    let s = size.clamp(12, 256);
    let f = s as f32;
    let c = f / 2.0;
    let mut rgba = vec![0u8; (s * s * 4) as usize];
    for y in 0..s {
        for x in 0..s {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let d = ((px - c).powi(2) + (py - c).powi(2)).sqrt();
            // Anti-aliased disc edge.
            let cover = (c - d).clamp(0.0, 1.0);
            if cover == 0.0 {
                continue;
            }
            // The microphone: a capsule (body), the stand below it.
            let (dx, top, bottom) = ((px - c).abs(), 0.22 * f, 0.58 * f);
            let r = 0.12 * f;
            let body = dx <= r && py >= top + r && py <= bottom - r
                || ((px - c).powi(2) + (py - (top + r)).powi(2)).sqrt() <= r
                || ((px - c).powi(2) + (py - (bottom - r)).powi(2)).sqrt() <= r;
            let stand = (dx <= 0.04 * f && py > bottom && py <= 0.74 * f)
                || (dx <= 0.16 * f && (0.72 * f..=0.78 * f).contains(&py));
            let base = if body || stand { MIC_WHITE } else { MIC_RED };
            let at = ((y * s + x) * 4) as usize;
            for i in 0..4 {
                rgba[at + i] = (base[i] as f32 * cover).round() as u8;
            }
        }
    }
    Picture { width: s, height: s, rgba }
}

/// The voice chat icons drawn above a blockhead (API 0.3.4 `ui.overhead-icon`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Overhead {
    Speaking,
    NoVoice,
    SelfMute,
    Deafen,
    AdminMute,
}

impl Overhead {
    pub const ALL: [Overhead; 5] =
        [Overhead::Speaking, Overhead::NoVoice, Overhead::SelfMute, Overhead::Deafen, Overhead::AdminMute];

    /// Disc colour, whether the glyph is a microphone (else headphones), whether it is crossed out.
    fn look(self) -> ([u8; 4], bool, bool) {
        match self {
            Overhead::Speaking => ([46, 160, 90, 255], true, false),
            Overhead::NoVoice => ([96, 96, 96, 255], false, true),
            Overhead::SelfMute => ([210, 130, 30, 255], true, true),
            Overhead::Deafen => ([210, 130, 30, 255], false, true),
            Overhead::AdminMute => (MIC_RED, true, true),
        }
    }
}

/// An overhead icon, `size` x `size` pixels: a disc with a white microphone or headphones, crossed out for the mute
/// and "no voice" states.
pub fn overhead_picture(kind: Overhead, size: u32) -> Picture {
    let s = size.clamp(12, 256);
    let f = s as f32;
    let c = f / 2.0;
    let (bg, mic, crossed) = kind.look();
    let mut rgba = vec![0u8; (s * s * 4) as usize];
    for y in 0..s {
        for x in 0..s {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let d = ((px - c).powi(2) + (py - c).powi(2)).sqrt();
            let cover = (c - d).clamp(0.0, 1.0);
            if cover == 0.0 {
                continue;
            }
            let dx = (px - c).abs();
            let glyph = if mic {
                let (top, bottom, r) = (0.22 * f, 0.58 * f, 0.12 * f);
                dx <= r && py >= top + r && py <= bottom - r
                    || ((px - c).powi(2) + (py - (top + r)).powi(2)).sqrt() <= r
                    || ((px - c).powi(2) + (py - (bottom - r)).powi(2)).sqrt() <= r
                    || (dx <= 0.04 * f && py > bottom && py <= 0.74 * f)
                    || (dx <= 0.16 * f && (0.72 * f..=0.78 * f).contains(&py))
            } else {
                // Headphones: the band (upper half ring) and two ear cups.
                let band = py <= 0.55 * f
                    && (0.24 * f..=0.31 * f).contains(&((px - c).powi(2) + (py - 0.55 * f).powi(2)).sqrt());
                let cup = (0.17 * f..=0.33 * f).contains(&dx) && (0.5 * f..=0.75 * f).contains(&py);
                band || cup
            };
            // The slash, top-left to bottom-right, with a dark edge.
            let slash = crossed && ((px - py).abs() / std::f32::consts::SQRT_2) <= 0.06 * f && d <= 0.42 * f;
            let base = match (slash, glyph) {
                (true, _) => [40, 20, 20, 255],
                (false, true) => MIC_WHITE,
                (false, false) => bg,
            };
            let at = ((y * s + x) * 4) as usize;
            for i in 0..4 {
                rgba[at + i] = (base[i] as f32 * cover).round() as u8;
            }
        }
    }
    Picture { width: s, height: s, rgba }
}

/// A short message: `text` on the counter's box, `scale` as for [`hud_scale`].
pub fn message_sprite(text: &str, scale: u32, serial: u64, at: (f32, f32)) -> Sprite {
    let p = render_label(text, scale);
    Sprite { overlay: Overlay { width: p.width, height: p.height, serial, rgba: Arc::new(p.rgba) }, at }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mic_indicator_is_a_red_disc_with_a_white_mic() {
        let p = mic_picture(64);
        assert_eq!((p.width, p.height), (64, 64));
        assert_eq!(p.pixel(0, 0), [0; 4], "outside the disc");
        assert_eq!(p.pixel(32, 24), MIC_WHITE, "the capsule");
        assert_eq!(p.pixel(32, 44), MIC_WHITE, "the stand");
        assert_eq!(p.pixel(10, 32), MIC_RED);
        let edge = p.pixel(32, 0);
        assert!(edge[3] > 0 && edge[3] < 255, "anti-aliased edge {edge:?}");
        assert_eq!(mic_picture(1).width, 12);
    }

    #[test]
    fn the_font_has_digits_and_fps() {
        for c in "0123456789FPS".chars() {
            let g = glyph(c).unwrap();
            assert!(g.iter().all(|r| *r < 32), "{c} fits 5 columns");
            assert!(g.iter().any(|r| *r != 0), "{c} is not blank");
        }
        assert_eq!(glyph(' '), None);
        assert_eq!(glyph('#'), None);
        let all: Vec<_> = "0123456789FPS".chars().map(|c| glyph(c).unwrap()).collect();
        for (i, a) in all.iter().enumerate() {
            assert!(all[i + 1..].iter().all(|b| b != a), "glyphs are distinct");
        }
    }

    #[test]
    fn labels_are_light_text_on_a_dark_rounded_box() {
        let p = render_label("1", 2);
        // 1 glyph: 5*2 wide plus 6 px padding each side; 7*2 tall plus 6 px each side.
        assert_eq!((p.width, p.height), (22, 26));
        assert_eq!(p.rgba.len(), 22 * 26 * 4);
        let box_a = (BOX_ALPHA * 255.0).round() as u8;
        // Corners are cut, edges' middles are box.
        assert_eq!(p.pixel(0, 0), [0, 0, 0, 0]);
        assert_eq!(p.pixel(21, 25), [0, 0, 0, 0]);
        assert_eq!(p.pixel(0, 13), [0, 0, 0, box_a]);
        assert_eq!(p.pixel(11, 0), [0, 0, 0, box_a]);
        let edge = p.pixel(1, 2)[3];
        assert!(edge > 0 && edge < box_a, "antialiased corner {edge}");
        // The "1": its stem is column 2 of the glyph (x = 6 + 4..6), every row; the top-left glyph cell is blank.
        for y in 6..20 {
            assert_eq!(p.pixel(10, y), TEXT, "stem at y {y}");
        }
        assert_eq!(p.pixel(6, 6), [0, 0, 0, box_a]);
        // Bottom serif spans glyph columns 1..=3.
        assert_eq!(p.pixel(8, 19), TEXT);
        assert_eq!(p.pixel(13, 19), TEXT);
        // Premultiplied: colour never exceeds alpha.
        assert!(p.rgba.chunks(4).all(|px| px[0] <= px[3]));
    }

    #[test]
    fn layout_scales_and_skips_unknown_characters() {
        let a = render_label("60 FPS", 1);
        assert_eq!((a.width, a.height), (6 * 6 - 1 + 6, 13));
        let b = render_label("60 FPS", 3);
        assert_eq!((b.width, b.height), (3 * (6 * 6 - 1) + 18, 39));
        // Unknown characters advance like a space; a zero scale behaves like 1.
        assert_eq!(render_label("6?", 0).width, render_label("6 ", 1).width);
        let lit = |p: &Picture| p.rgba.chunks(4).filter(|px| px[..] == TEXT).count();
        assert_eq!(lit(&render_label("6?", 1)), lit(&render_label("6", 1)));
        let empty = render_label("", 2);
        assert_eq!((empty.width, empty.height), (12, 26));
        assert_eq!(lit(&empty), 0);
    }

    #[test]
    fn counter_text_scale_and_picture() {
        assert_eq!(fps_label(59.6), "60 FPS");
        assert_eq!(fps_label(-3.0), "0 FPS");
        assert_eq!(fps_label(143.2), "143 FPS");
        assert_eq!([hud_scale(0), hud_scale(768), hud_scale(1080), hud_scale(2160), hud_scale(99999)], [2, 2, 3, 6, 8]);
        let h = fps_hud(30.0, 7, 1080, Corner::TopLeft);
        assert_eq!(h.overlay.serial, 7);
        assert_eq!(h.margin, 12);
        let p = render_label("30 FPS", 3);
        assert_eq!((h.overlay.width, h.overlay.height), (p.width, p.height));
        assert_eq!(*h.overlay.rgba, p.rgba);
    }

    #[test]
    fn the_counter_sits_in_its_corner_of_the_game_picture() {
        assert_eq!(Corner::default(), Corner::TopLeft);
        let names = ["tl", "tr", "bl", "br"].map(|s| Corner::parse(s).unwrap());
        assert_eq!(names, [Corner::TopLeft, Corner::TopRight, Corner::BottomLeft, Corner::BottomRight]);
        assert_eq!(Corner::parse("middle"), None);
        // A 1080p picture letterboxed at x 240 in a 2400-wide window; the counter is 123x39 with a 12 px margin.
        let vp = (240, 0, 1920, 1080);
        let at = |corner| {
            let mut h = fps_hud(30.0, 1, 1080, corner);
            h.overlay.width = 123;
            h.origin(vp)
        };
        assert_eq!(at(Corner::TopLeft), (252, 1080 - 12 - 39));
        assert_eq!(at(Corner::TopRight), (240 + 1920 - 12 - 123, 1080 - 12 - 39));
        assert_eq!(at(Corner::BottomLeft), (252, 12));
        assert_eq!(at(Corner::BottomRight), (240 + 1920 - 12 - 123, 12));
    }

    #[test]
    fn the_font_has_letters_and_punctuation() {
        let set = "ABCDEFGHIJKLMNOPQRSTUVWXYZ-+.:/()!";
        let all: Vec<_> = set.chars().filter_map(glyph).collect();
        assert_eq!(all.len(), set.len(), "every character has a glyph");
        for (i, a) in all.iter().enumerate() {
            assert!(a.iter().all(|r| *r < 32) && a.iter().any(|r| *r != 0));
            assert!(all[i + 1..].iter().all(|b| b != a), "glyphs are distinct");
        }
        assert_eq!(glyph('h'), glyph('H'));
        assert_ne!(glyph('O'), glyph('0'));
    }

    #[test]
    fn the_font_has_turkish_russian_and_kazakh_letters() {
        let set = "ÇĞİÖŞÜБГДЁЖЗИЙЛПУФЦЧШЩЪЫЬЭЮЯӘҒҚҢӨҰҺ,'";
        let all: Vec<_> = set.chars().filter_map(glyph).collect();
        assert_eq!(all.len(), set.chars().count(), "every character has a glyph");
        let latin: Vec<_> = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".chars().filter_map(glyph).collect();
        for (i, a) in all.iter().enumerate() {
            assert!(a.iter().all(|r| *r < 32) && a.iter().any(|r| *r != 0));
            assert!(all[i + 1..].iter().all(|b| b != a), "glyphs are distinct");
            assert!(!latin.contains(a), "{:?} differs from the Latin letters", set.chars().nth(i));
        }
        // Look-alikes share the Latin glyphs; lower case is drawn as upper case.
        for (cy, la) in [('А', 'A'), ('Н', 'H'), ('Р', 'P'), ('С', 'C'), ('Ү', 'Y'), ('І', 'I'), ('ы', 'Ы'), ('ç', 'Ç')]
        {
            assert_eq!(glyph(cy), glyph(la), "{cy}");
        }
        assert_eq!(glyph('’'), glyph('\''));
        assert_eq!(glyph('ı'), glyph('I'));
    }

    #[test]
    fn host_messages_follow_the_language() {
        assert_eq!(
            [Lang::parse("en"), Lang::parse("tr"), Lang::parse("ru-RU"), Lang::parse("kk_KZ")],
            [Lang::En, Lang::Tr, Lang::Ru, Lang::Kk]
        );
        assert_eq!([Lang::parse("RU"), Lang::parse("de"), Lang::parse("")], [Lang::Ru, Lang::En, Lang::En]);
        assert_eq!(Lang::default(), Lang::En);
        assert_eq!(localise(Lang::Ru, "GAME BUTTONS HIDDEN"), "КНОПКИ ИГРЫ СКРЫТЫ");
        assert_eq!(localise(Lang::En, "COPIED TO CLIPBOARD"), "COPIED TO CLIPBOARD");
        assert_eq!(localise(Lang::Kk, "PHOTO SAVED: /home/a/b.png"), "ФОТО САҚТАЛДЫ: /home/a/b.png");
        assert_eq!(localise(Lang::Tr, "60 FPS"), "60 FPS");
        // Every translation is drawable: no character falls back to a blank.
        for text in MESSAGES.iter().flatten().chain(&PHOTO_SAVED) {
            for c in text.chars().filter(|c| *c != ' ') {
                assert!(glyph(c).is_some(), "{c:?} in {text:?}");
            }
        }
    }

    #[test]
    fn sprites_centre_on_their_spot_of_the_game_picture() {
        let p = cursor_picture(40);
        let s = Sprite {
            overlay: Overlay { width: p.width, height: p.height, serial: 1, rgba: Arc::new(p.rgba) },
            at: (0.25, 0.5),
        };
        // A 1000x500 picture at x 100: the centre is at x 100 + 250, y 250 from the bottom.
        assert_eq!(s.origin((100, 0, 1000, 500)), (330, 230));
        let top = Sprite { at: (0.5, 0.0), ..s.clone() };
        assert_eq!(top.origin((0, 0, 800, 600)), (380, 580));
        let m = message_sprite("HIDDEN", 2, 9, (0.5, 0.1));
        assert_eq!((m.overlay.serial, m.at), (9, (0.5, 0.1)));
        assert_eq!((m.overlay.width, m.overlay.height), (render_label("HIDDEN", 2).width, 26));
    }

    #[test]
    fn the_cursor_is_corner_brackets_and_a_dot_with_an_outline() {
        let p = cursor_picture(32);
        assert_eq!((p.width, p.height), (32, 32));
        // Corners: light 1 px in from the edge, outlined outside; the middle of an edge is empty.
        assert_eq!(p.pixel(1, 1), CURSOR_LIGHT);
        assert_eq!(p.pixel(0, 1), CURSOR_DARK);
        assert_eq!(p.pixel(30, 30), CURSOR_LIGHT);
        assert_eq!(p.pixel(16, 1), [0, 0, 0, 0]);
        assert_eq!(p.pixel(16, 16), CURSOR_LIGHT);
        assert_eq!(p.pixel(8, 16), [0, 0, 0, 0]);
        // Sizes are clamped.
        assert_eq!(cursor_picture(2).width, 12);
        assert_eq!(cursor_picture(9999).width, 512);
    }
}
