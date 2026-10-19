//! The heads-up FPS counter: a tiny built-in 5x7 bitmap font (digits, "FPS" and space) rendered as light text on a
//! semi-transparent dark rounded box. The picture is premultiplied RGBA, top row first, ready to be blended over the
//! game picture by the window (`vsurface`).

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
/// (they are drawn as blanks).
pub fn glyph(c: char) -> Option<[u8; 7]> {
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
        _ => return None,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_font_has_digits_and_fps() {
        for c in "0123456789FPS".chars() {
            let g = glyph(c).unwrap();
            assert!(g.iter().all(|r| *r < 32), "{c} fits 5 columns");
            assert!(g.iter().any(|r| *r != 0), "{c} is not blank");
        }
        assert_eq!(glyph(' '), None);
        assert_eq!(glyph('x'), None);
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
}
