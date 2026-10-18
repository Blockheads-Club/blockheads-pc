//! Software renderer for the Java view tree (`com.apportable.ui`).
//!
//! On Android these are real views drawn over the game's GL surface. Here the tree lives in [`crate::ui`]; this module
//! turns it into a list of draw items (fills, bitmaps, text) placed in window pixels ([`Vm::ui_offset`] centres the
//! views' portrait space on the window, exactly as [`Vm::ui_touch`] hit-tests it) and rasterises them into an RGBA
//! buffer the size of the window. The runner draws the result over the game's frame.
//!
//! The result is cached: while the item list stays the same the same [`UiFrame`] (same `serial`) is returned, so the
//! window only uploads a texture when something changed.

use crate::jlib::{bitmap_info_in, face_in, Face, BOLD, ITALIC};
use crate::ui::{TextStyle, View};
use crate::value::Obj;
use crate::vm::{State, Vm};
use ab_glyph::{point, Font, FontArc, PxScale, ScaleFont};
use std::collections::HashMap;
use std::sync::Arc;

/// The rendered overlay: premultiplied RGBA, top row first, one pixel per window pixel.
#[derive(Debug, PartialEq)]
pub struct UiFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Changes whenever the pixels do.
    pub serial: u64,
}

/// `[left, top, right, bottom]`.
type Rect = [f32; 4];

fn intersect(a: Rect, b: Rect) -> Rect {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]
}

/// A bitmap copied out of guest memory (premultiplied RGBA).
#[derive(Debug, PartialEq)]
struct Bmp {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

/// A parsed font and what identifies it.
#[derive(Debug, Clone)]
struct FontRef {
    key: String,
    font: FontArc,
}

impl PartialEq for FontRef {
    fn eq(&self, o: &FontRef) -> bool {
        self.key == o.key
    }
}

type Sf<'a> = ab_glyph::PxScaleFont<&'a FontArc>;

/// One thing to draw. `clip` is the area the item may touch.
#[derive(Debug, Clone, PartialEq)]
enum Item {
    Fill { rect: Rect, clip: Rect, argb: u32, alpha: f32 },
    Image { rect: Rect, clip: Rect, alpha: f32, bmp: Arc<Bmp> },
    Text { rect: Rect, clip: Rect, alpha: f32, text: String, font: FontRef, style: TextStyle },
}

/// The last rendered frame and what it was made from.
struct Last {
    items: Vec<Item>,
    size: (u32, u32),
    frame: Arc<UiFrame>,
}

/// What [`Vm::render_ui`] remembers between frames.
#[derive(Default)]
pub(crate) struct RenderCache {
    fonts: HashMap<String, Option<FontArc>>,
    last: Option<Last>,
    serial: u64,
}

/// `Button` control states of UIKit (`UIControlState`).
const STATE_NORMAL: i32 = 0;
const STATE_HIGHLIGHTED: i32 = 1;
const STATE_DISABLED: i32 = 2;

fn state_bitmap(list: &[(i32, Obj)], state: i32) -> Option<Obj> {
    let find = |s: i32| list.iter().find(|b| b.0 == s).map(|b| b.1);
    find(state).or_else(|| find(STATE_NORMAL))
}

/// The font for `face`: the asset the game named, else the bundled stand-in with the style bits.
fn load_font(vm: &Vm, cache: &mut HashMap<String, Option<FontArc>>, face: &Face) -> FontRef {
    let bundled = |style: i32| {
        let s = if style & BOLD != 0 {
            bhpc_vfs::fonts::Style::Bold
        } else if style & ITALIC != 0 {
            bhpc_vfs::fonts::Style::Italic
        } else {
            bhpc_vfs::fonts::Style::Regular
        };
        (format!("bundled:{}", s as u8), FontArc::try_from_vec(bhpc_vfs::fonts::bundled(s).to_vec()).ok())
    };
    let (key, font) = if face.path.is_empty() {
        let (k, _) = bundled(face.style);
        let font = cache.get(&k).cloned().unwrap_or_else(|| bundled(face.style).1);
        (k, font)
    } else {
        let key = format!("file:{}", face.path);
        let font = cache.get(&key).cloned().unwrap_or_else(|| {
            let base = face.path.rsplit('/').next().unwrap_or(&face.path);
            let data = vm.assets().read(&face.path).or_else(|| vm.assets().read(base));
            data.and_then(|d| FontArc::try_from_vec(d).ok())
        });
        (key, font)
    };
    // `bundled` is the last resort, also for an asset that did not parse.
    let font = font.or_else(|| bundled(face.style).1);
    cache.insert(key.clone(), font.clone());
    FontRef { key, font: font.expect("the bundled fonts parse") }
}

impl Vm {
    /// Draw the view tree. `None` when nothing visible is drawn (no window, everything hidden or transparent), so the
    /// caller can skip the overlay. The size is the window size: [`Vm::screen_size`].
    pub fn render_ui(&self) -> Option<Arc<UiFrame>> {
        let (sw, sh) = self.screen_size();
        let size = (sw.max(1) as u32, sh.max(1) as u32);
        let mut cache = std::mem::take(&mut self.st().render);
        let items = self.plan_ui(&mut cache);
        let frame = if items.is_empty() {
            None
        } else if let Some(l) = cache.last.as_ref().filter(|l| l.items == items && l.size == size) {
            Some(l.frame.clone())
        } else {
            cache.serial += 1;
            let mut canvas = Canvas { w: size.0, h: size.1, px: vec![0; size.0 as usize * size.1 as usize * 4] };
            for i in &items {
                canvas.draw(i);
            }
            let f = Arc::new(UiFrame { width: size.0, height: size.1, rgba: canvas.px, serial: cache.serial });
            cache.last = Some(Last { items, size, frame: f.clone() });
            Some(f)
        };
        if frame.is_none() {
            cache.last = None;
        }
        self.st().render = cache;
        frame
    }

    /// Everything the windows draw, back to front.
    fn plan_ui(&self, cache: &mut RenderCache) -> Vec<Item> {
        let st = self.st();
        let mut windows: Vec<Obj> =
            st.ui.views.iter().filter(|(_, v)| v.kind == "Window" && v.parent.is_none()).map(|(o, _)| *o).collect();
        windows.sort();
        let mut out = Vec::new();
        let (sw, sh) = self.screen_size();
        let screen = [0.0, 0.0, sw as f32, sh as f32];
        let (ox, oy) = self.ui_offset();
        // The portrait space the game lays its views out in, in window pixels.
        let space = (sw.min(sh) as f32, sw.max(sh) as f32);
        let space = [ox, oy, ox + space.0, oy + space.1];
        for w in windows {
            let Some(root) = st.ui.views.get(&w) else { continue };
            let mut pl = Planner { vm: self, st: &st, fonts: &mut cache.fonts, out: &mut out, screen, space };
            pl.walk(w, (root.frame[0] + ox, root.frame[1] + oy), screen, 1.0, 0);
        }
        out
    }
}

struct Planner<'a> {
    vm: &'a Vm,
    st: &'a State,
    fonts: &'a mut HashMap<String, Option<FontArc>>,
    out: &'a mut Vec<Item>,
    /// The window and the portrait space of the views, in window pixels.
    screen: Rect,
    space: Rect,
}

/// Views nest a few levels; this only guards against a cycle that `ui_attach` should never build.
const MAX_DEPTH: usize = 64;

impl Planner<'_> {
    fn bitmap(&self, b: Obj) -> Option<Arc<Bmp>> {
        let (w, h, ptr) = bitmap_info_in(self.st, b)?;
        let px = self.vm.mem().read_bytes(ptr, w * h * 4).ok()?;
        Some(Arc::new(Bmp { w, h, px }))
    }

    fn walk(&mut self, v: Obj, origin: (f32, f32), clip: Rect, alpha: f32, depth: usize) {
        let views: &HashMap<Obj, View> = &self.st.ui.views;
        let Some(info) = views.get(&v) else { return };
        if info.hidden || info.alpha <= 0.01 || depth > MAX_DEPTH {
            return;
        }
        let alpha = alpha * info.alpha;
        let rect =
            [origin.0, origin.1, origin.0 + info.frame[2] - info.frame[0], origin.1 + info.frame[3] - info.frame[1]];
        let window = info.kind == "Window";
        let clip = if info.clips && !window { intersect(clip, rect) } else { clip };
        if !window {
            self.draw_view(info, rect, clip, alpha);
        }
        for c in &info.children {
            let Some(ci) = views.get(c) else { continue };
            let co = (origin.0 + ci.frame[0] - info.bounds[0], origin.1 + ci.frame[1] - info.bounds[1]);
            self.walk(*c, co, clip, alpha, depth + 1);
        }
    }

    fn draw_view(&mut self, info: &View, rect: Rect, clip: Rect, alpha: f32) {
        if let Some(argb) = info.bg.filter(|c| c >> 24 != 0) {
            // A scrim that covers the whole portrait space covers the whole window (the space is smaller than it).
            let s = self.space;
            let full = rect[0] <= s[0] + 0.5 && rect[1] <= s[1] + 0.5 && rect[2] >= s[2] - 0.5 && rect[3] >= s[3] - 0.5;
            self.out.push(Item::Fill { rect: if full { self.screen } else { rect }, clip, argb, alpha });
        }
        let mut images: Vec<Obj> = Vec::new();
        match info.kind.as_str() {
            "ImageView" => images.extend(info.image.map(|i| i.0)),
            "Button" => {
                let state = if !info.enabled {
                    STATE_DISABLED
                } else if info.highlighted {
                    STATE_HIGHLIGHTED
                } else {
                    STATE_NORMAL
                };
                images.extend(state_bitmap(&info.backgrounds, state));
            }
            _ => {}
        }
        for b in images {
            if let Some(bmp) = self.bitmap(b) {
                self.out.push(Item::Image { rect, clip, alpha, bmp });
            }
        }
        let has_caption = info.kind == "Button" && info.children.is_empty();
        let drawn = matches!(info.kind.as_str(), "Label" | "TextField" | "TextView") || has_caption;
        if drawn && !info.text.is_empty() {
            let face = info.style.font.map(|t| face_in(self.st, t)).unwrap_or_default();
            let font = load_font(self.vm, self.fonts, &face);
            let style = TextStyle { font: None, ..info.style.clone() };
            self.out.push(Item::Text { rect, clip, alpha, text: info.text.clone(), font, style });
        }
    }
}

/// Premultiplied RGBA pixels.
struct Canvas {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

/// Source-over for premultiplied pixels.
fn over(dst: &mut [u8], src: [u8; 4]) {
    let inv = 255 - u32::from(src[3]);
    for (d, s) in dst.iter_mut().zip(src) {
        *d = (u32::from(s) + (u32::from(*d) * inv + 127) / 255).min(255) as u8;
    }
}

fn premultiply(argb: u32, alpha: f32) -> [u8; 4] {
    let a = ((argb >> 24) & 0xFF) as f32 / 255.0 * alpha.clamp(0.0, 1.0);
    let ch = |shift: u32| (((argb >> shift) & 0xFF) as f32 * a).round() as u8;
    [ch(16), ch(8), ch(0), (a * 255.0).round() as u8]
}

/// Pixel bounds `x0, y0, x1, y1` of a rectangle limited to `clip` and the canvas.
fn pixel_bounds(rect: Rect, clip: Rect, w: u32, h: u32) -> (i32, i32, i32, i32) {
    let r = intersect(rect, clip);
    let c = |v: f32, max: u32| (v.round().clamp(0.0, max as f32)) as i32;
    (c(r[0], w), c(r[1], h), c(r[2], w), c(r[3], h))
}

impl Canvas {
    fn draw(&mut self, item: &Item) {
        match item {
            Item::Fill { rect, clip, argb, alpha } => {
                let src = premultiply(*argb, *alpha);
                let (x0, y0, x1, y1) = pixel_bounds(*rect, *clip, self.w, self.h);
                for y in y0..y1 {
                    for x in x0..x1 {
                        self.blend(x, y, src);
                    }
                }
            }
            Item::Image { rect, clip, alpha, bmp } => self.image(*rect, *clip, *alpha, bmp),
            Item::Text { rect, clip, alpha, text, font, style } => {
                self.text(*rect, *clip, *alpha, text, &font.font, style)
            }
        }
    }

    fn blend(&mut self, x: i32, y: i32, src: [u8; 4]) {
        let i = (y as usize * self.w as usize + x as usize) * 4;
        over(&mut self.px[i..i + 4], src);
    }

    /// The bitmap stretched over `rect` (area-averaged when shrunk).
    fn image(&mut self, rect: Rect, clip: Rect, alpha: f32, bmp: &Bmp) {
        let (rw, rh) = (rect[2] - rect[0], rect[3] - rect[1]);
        if rw <= 0.0 || rh <= 0.0 || bmp.w == 0 || bmp.h == 0 {
            return;
        }
        let (x0, y0, x1, y1) = pixel_bounds(rect, clip, self.w, self.h);
        let (sx, sy) = (bmp.w as f32 / rw, bmp.h as f32 / rh);
        // Samples per axis: enough to average a shrunk image without a blur for 1:1.
        let (nx, ny) = ((sx.ceil() as u32).clamp(1, 4), (sy.ceil() as u32).clamp(1, 4));
        let a = alpha.clamp(0.0, 1.0);
        for y in y0..y1 {
            for x in x0..x1 {
                let mut acc = [0f32; 4];
                for j in 0..ny {
                    for i in 0..nx {
                        let u = (x as f32 + (i as f32 + 0.5) / nx as f32 - rect[0]) * sx - 0.5;
                        let v = (y as f32 + (j as f32 + 0.5) / ny as f32 - rect[1]) * sy - 0.5;
                        let p = bmp.bilinear(u, v);
                        for (s, c) in acc.iter_mut().zip(p) {
                            *s += c;
                        }
                    }
                }
                let n = (nx * ny) as f32;
                self.blend(x, y, acc.map(|c| (c / n * a).round().clamp(0.0, 255.0) as u8));
            }
        }
    }

    fn text(&mut self, rect: Rect, clip: Rect, alpha: f32, text: &str, font: &FontArc, st: &TextStyle) {
        let width = rect[2] - rect[0];
        let unit = font.height_unscaled() / font.units_per_em().unwrap_or(1000.0);
        let scaled = |size: f32| font.as_scaled(PxScale::from(size * unit));
        // A single line that is too wide shrinks down to the minimum size before it is cut.
        let mut size = st.size.max(1.0);
        if st.lines == 1 && st.min_size > 0.0 {
            while size > st.min_size && measure(&scaled(size), text) > width {
                size = (size - 0.5).max(st.min_size);
            }
        }
        let sf = scaled(size);
        let lines = wrap(&sf, text, width, st.lines);
        let line_h = sf.height() + sf.line_gap();
        let top = rect[1] + ((rect[3] - rect[1]) - line_h * lines.len() as f32).max(0.0) / 2.0;
        let clip = intersect(clip, [f32::MIN, f32::MIN, f32::MAX, f32::MAX]);
        let layers = [(st.shadow_offset, st.shadow_color, st.shadow_color >> 24 != 0), ([0.0; 2], st.color, true)];
        for (off, color, on) in layers {
            if !on {
                continue;
            }
            let rgba = premultiply(color, alpha);
            for (n, line) in lines.iter().enumerate() {
                let lw = measure(&sf, line);
                let x = match st.align {
                    1 => rect[0] + (width - lw) / 2.0,
                    2 => rect[2] - lw,
                    _ => rect[0],
                };
                let baseline = top + sf.ascent() + line_h * n as f32;
                self.glyphs(&sf, line, (x + off[0], baseline + off[1]), clip, rgba);
            }
        }
    }

    fn glyphs(&mut self, sf: &Sf<'_>, line: &str, start: (f32, f32), clip: Rect, rgba: [u8; 4]) {
        let mut x = start.0;
        let mut prev = None;
        for c in line.chars() {
            let id = sf.glyph_id(c);
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            let g = id.with_scale_and_position(sf.scale(), point(x, start.1));
            x += sf.h_advance(id);
            prev = Some(id);
            let Some(o) = sf.font().outline_glyph(g) else { continue };
            let b = o.px_bounds();
            o.draw(|gx, gy, cov| {
                let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                let inside = px >= 0 && py >= 0 && px < self.w as i32 && py < self.h as i32;
                let in_clip = (px as f32 + 0.5) >= clip[0]
                    && (px as f32 + 0.5) < clip[2]
                    && (py as f32 + 0.5) >= clip[1]
                    && (py as f32 + 0.5) < clip[3];
                if inside && in_clip && cov > 0.0 {
                    self.blend(px, py, rgba.map(|c| (f32::from(c) * cov.min(1.0)).round() as u8));
                }
            });
        }
    }
}

impl Bmp {
    fn texel(&self, x: i32, y: i32) -> [f32; 4] {
        let (x, y) = (x.clamp(0, self.w as i32 - 1) as usize, y.clamp(0, self.h as i32 - 1) as usize);
        let i = (y * self.w as usize + x) * 4;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]].map(f32::from)
    }

    fn bilinear(&self, u: f32, v: f32) -> [f32; 4] {
        let (x0, y0) = (u.floor(), v.floor());
        let (fx, fy) = (u - x0, v - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let (a, b, c, d) =
            (self.texel(x0, y0), self.texel(x0 + 1, y0), self.texel(x0, y0 + 1), self.texel(x0 + 1, y0 + 1));
        std::array::from_fn(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
    }
}

/// Advance width of `s` (with kerning).
fn measure(sf: &Sf<'_>, s: &str) -> f32 {
    let mut w = 0.0;
    let mut prev = None;
    for c in s.chars() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Break `text` into lines of at most `width`: at newlines, then at spaces (a word longer than a line is split).
/// `max_lines > 0` limits the count; the last line then ends in `...` if text was cut.
fn wrap(sf: &Sf<'_>, text: &str, width: f32, max_lines: i32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        for word in para.split(' ') {
            let joined = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if cur.is_empty() || measure(sf, &joined) <= width {
                cur = joined;
            } else {
                lines.push(std::mem::take(&mut cur));
                cur = word.to_string();
            }
            // A single word wider than the line: split it by characters.
            while measure(sf, &cur) > width && cur.chars().count() > 1 {
                let mut cut = cur.chars().count() - 1;
                while cut > 1 && measure(sf, &cur.chars().take(cut).collect::<String>()) > width {
                    cut -= 1;
                }
                lines.push(cur.chars().take(cut).collect());
                cur = cur.chars().skip(cut).collect();
            }
        }
        lines.push(cur);
    }
    if max_lines > 0 && lines.len() > max_lines as usize {
        lines.truncate(max_lines as usize);
        let last = &mut lines[max_lines as usize - 1];
        last.push_str("...");
        while measure(sf, last) > width && last.chars().count() > 4 {
            let n = last.chars().count();
            *last = last.chars().take(n - 4).collect::<String>() + "...";
        }
    }
    lines
}
