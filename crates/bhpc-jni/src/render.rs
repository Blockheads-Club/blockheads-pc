//! Software renderer for the Java view tree (`com.apportable.ui`).
//!
//! On Android these are real views drawn over the game's GL surface. Here the tree lives in [`crate::ui`]; this module
//! turns it into a list of draw items (fills, bitmaps, text) placed in window pixels ([`Vm::ui_offset`] centres the
//! views' portrait space on the window, exactly as [`Vm::ui_touch`] hit-tests it) and rasterises them into an RGBA
//! buffer the size of the window. The runner draws the result over the game's frame.
//!
//! The result is cached: while the item list stays the same the same [`UiFrame`] (same `serial`) is returned, so the
//! window only uploads a texture when something changed. The list is not even rebuilt while nothing the views are drawn
//! from changed (the view tree's generation, see [`Vm::ui_changed`], and the window size), except every
//! [`RECHECK_FRAMES`] frames as a safety net. When it did change, only the area of the items that differ is drawn again
//! over a copy of the last picture.

use crate::jlib::{bitmap_info_in, face_in, Face, BOLD, ITALIC};
use crate::ui::{TextStyle, View};
use crate::value::Obj;
use crate::vm::{State, Vm};
use ab_glyph::{point, Font, FontArc, PxScale, ScaleFont};
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) mod dialog;

/// The rendered overlay: premultiplied RGBA, top row first, one pixel per window pixel.
#[derive(Debug, PartialEq)]
pub struct UiFrame {
    pub width: u32,
    pub height: u32,
    /// Shared with the window's overlay, so handing it over copies nothing.
    pub rgba: Arc<Vec<u8>>,
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

/// Receives the lines of a laid out text: font, line, pen start, premultiplied colour.
type TextRun<'r> = dyn FnMut(&Sf<'_>, &str, (f32, f32), [u8; 4]) + 'r;

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

/// A frame answered without rebuilding the item list is checked again after this many frames anyway, in case the views'
/// pixels changed in a way [`Vm::ui_changed`] did not hear of.
pub(crate) const RECHECK_FRAMES: u32 = 60;

/// What [`Vm::render_ui`] remembers between frames.
#[derive(Default)]
pub(crate) struct RenderCache {
    fonts: HashMap<String, Option<FontArc>>,
    last: Option<Last>,
    serial: u64,
    /// The view tree generation and window size the last answer (`last`, or no frame) was made for.
    seen: Option<(u64, (u32, u32))>,
    /// Frames answered from `seen` since the list was last rebuilt.
    skipped: u32,
    /// Rebuild the list and draw the whole picture every time (`Vm::set_ui_dirty_tracking(false)`).
    pub(crate) always_full: bool,
    /// The pixels of the frame before the last one: once the window let go of them, the next frame is drawn into
    /// them (a fresh buffer of a window's size costs more in page faults than drawing a popup).
    spare: Option<Arc<Vec<u8>>>,
}

impl RenderCache {
    /// A buffer of `len` bytes to draw into: the spare one when nobody else holds it (contents undefined).
    fn buffer(&mut self, len: usize) -> Vec<u8> {
        let mut px = self.spare.take().and_then(|a| Arc::try_unwrap(a).ok()).unwrap_or_default();
        px.resize(len, 0);
        px
    }
}

/// `Button` control states of UIKit (`UIControlState`).
const STATE_NORMAL: i32 = 0;
const STATE_HIGHLIGHTED: i32 = 1;
const STATE_DISABLED: i32 = 2;

fn state_bitmap(list: &[(i32, Obj)], state: i32) -> Option<Obj> {
    let find = |s: i32| list.iter().find(|b| b.0 == s).map(|b| b.1);
    find(state).or_else(|| find(STATE_NORMAL))
}

/// Cache key of the bundled stand-in for the style bits.
fn bundled_key(style: i32) -> String {
    let s = if style & BOLD != 0 {
        bhpc_vfs::fonts::Style::Bold
    } else if style & ITALIC != 0 {
        bhpc_vfs::fonts::Style::Italic
    } else {
        bhpc_vfs::fonts::Style::Regular
    };
    format!("bundled:{}", s as u8)
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
        (bundled_key(style), FontArc::try_from_vec(bhpc_vfs::fonts::bundled(s).to_vec()).ok())
    };
    let (key, font) = if face.path.is_empty() {
        let k = bundled_key(face.style);
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
        let (mut cache, generation) = {
            let mut st = self.st();
            (std::mem::take(&mut st.render), st.ui.generation)
        };
        let key = Some((generation, size));
        if !cache.always_full && cache.seen == key && cache.skipped < RECHECK_FRAMES {
            cache.skipped += 1;
            let frame = cache.last.as_ref().map(|l| l.frame.clone());
            self.st().render = cache;
            return frame;
        }
        (cache.seen, cache.skipped) = (key, 0);
        let mut items = self.plan_ui(&mut cache);
        items.extend(self.dialog_items());
        let frame = if items.is_empty() {
            None
        } else if let Some(l) = cache.last.as_ref().filter(|l| l.items == items && l.size == size) {
            Some(l.frame.clone())
        } else {
            cache.serial += 1;
            let mut px = cache.buffer(size.0 as usize * size.1 as usize * 4);
            let last = cache.last.take();
            let partial = last.as_ref().filter(|l| l.size == size && !cache.always_full);
            if !partial.is_some_and(|l| redraw_changed(l, &items, &mut px)) {
                px.fill(0);
                let mut canvas = Canvas::new(size.0, size.1, px);
                for i in &items {
                    canvas.draw(i);
                }
                px = canvas.px;
            }
            cache.spare = last.map(|l| l.frame.rgba.clone());
            let rgba = px;
            let f = Arc::new(UiFrame { width: size.0, height: size.1, rgba: Arc::new(rgba), serial: cache.serial });
            cache.last = Some(Last { items, size, frame: f.clone() });
            Some(f)
        };
        if frame.is_none() {
            // Kept to draw the next popup into.
            cache.spare = cache.last.take().map(|l| l.frame.rgba.clone()).or(cache.spare.take());
        }
        self.st().render = cache;
        frame
    }

    /// The renderer rebuilds and redraws everything each frame when `on` is false (an off switch for the change
    /// tracking, which is on by default).
    pub fn set_ui_dirty_tracking(&self, on: bool) {
        self.st().render.always_full = !on;
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

/// `x0, y0, x1, y1` in whole pixels.
type Area = (i32, i32, i32, i32);

fn union(a: Option<Area>, b: Area) -> Option<Area> {
    if b.0 >= b.2 || b.1 >= b.3 {
        return a;
    }
    Some(a.map_or(b, |a| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))))
}

/// Draw the picture of `items` into `px` by repainting, over a copy of `last`'s picture, only the area the items that
/// differ from `last`'s touch (the items between their common start and end). False (and `px` untouched) when that
/// area is most of the picture: drawing it all is as cheap.
fn redraw_changed(last: &Last, items: &[Item], px: &mut Vec<u8>) -> bool {
    redraw_area(last, items).is_some_and(|area| {
        let mut canvas = Canvas::new(last.size.0, last.size.1, std::mem::take(px));
        canvas.px.copy_from_slice(&last.frame.rgba);
        canvas.repaint(area, items);
        *px = canvas.px;
        true
    })
}

/// The area [`redraw_changed`] repaints, if it is small enough.
fn redraw_area(last: &Last, items: &[Item]) -> Option<Area> {
    let (old, new) = (&last.items[..], items);
    let start = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let end = old[start..].iter().rev().zip(new[start..].iter().rev()).take_while(|(a, b)| a == b).count();
    let (w, h) = last.size;
    let probe = Canvas::new(w, h, Vec::new());
    let changed = old[start..old.len() - end].iter().chain(&new[start..new.len() - end]);
    let area = changed.fold(None, |acc, i| union(acc, probe.extent(i)))?;
    let (x0, y0, x1, y1) = area;
    ((x1 - x0) as u64 * (y1 - y0) as u64 * 2 <= w as u64 * h as u64).then_some(area)
}

/// Premultiplied RGBA pixels.
struct Canvas {
    w: u32,
    h: u32,
    px: Vec<u8>,
    /// Only pixels inside this area (whole pixels) are drawn.
    limit: Rect,
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
    let (x0, y0) = (c(r[0], w), c(r[1], h));
    // An empty intersection is an empty area, not an inverted one.
    (x0, y0, c(r[2], w).max(x0), c(r[3], h).max(y0))
}

impl Canvas {
    fn new(w: u32, h: u32, px: Vec<u8>) -> Canvas {
        Canvas { w, h, px, limit: [0.0, 0.0, w as f32, h as f32] }
    }

    /// Clear `area` and draw `items` again inside it.
    fn repaint(&mut self, (x0, y0, x1, y1): Area, items: &[Item]) {
        self.limit = [x0 as f32, y0 as f32, x1 as f32, y1 as f32];
        for y in y0..y1 {
            let row = (y as usize * self.w as usize + x0 as usize) * 4;
            self.px[row..row + (x1 - x0) as usize * 4].fill(0);
        }
        for i in items {
            self.draw(i);
        }
    }

    /// The pixels `item` may touch (a superset: a glyph's whole box).
    fn extent(&self, item: &Item) -> Area {
        match item {
            Item::Fill { rect, clip, .. } | Item::Image { rect, clip, .. } => {
                pixel_bounds(*rect, *clip, self.w, self.h)
            }
            Item::Text { rect, clip, alpha, text, font, style } => {
                let mut area = None;
                Self::layout(*rect, *alpha, text, &font.font, style, &mut |sf, line, start, _| {
                    for b in glyph_boxes(sf, line, start) {
                        area = union(area, b);
                    }
                });
                let c = (clip[0].floor() as i32, clip[1].floor() as i32, clip[2].ceil() as i32, clip[3].ceil() as i32);
                let (w, h) = (self.w as i32, self.h as i32);
                area.map_or((0, 0, 0, 0), |a| {
                    (a.0.max(c.0).max(0), a.1.max(c.1).max(0), a.2.min(c.2).min(w), a.3.min(c.3).min(h))
                })
            }
        }
    }

    fn draw(&mut self, item: &Item) {
        match item {
            Item::Fill { rect, clip, argb, alpha } => {
                let src = premultiply(*argb, *alpha);
                let (x0, y0, x1, y1) = pixel_bounds(*rect, intersect(*clip, self.limit), self.w, self.h);
                for y in y0..y1 {
                    let row = (y as usize * self.w as usize) * 4;
                    for px in self.px[row + x0 as usize * 4..row + x1 as usize * 4].chunks_exact_mut(4) {
                        // Over a transparent or under an opaque source the result is the source itself.
                        if src[3] == 255 || px == [0; 4] {
                            px.copy_from_slice(&src);
                        } else {
                            over(px, src);
                        }
                    }
                }
            }
            Item::Image { rect, clip, alpha, bmp } => self.image(*rect, intersect(*clip, self.limit), *alpha, bmp),
            Item::Text { rect, clip, alpha, text, font, style } => {
                let clip = intersect(*clip, self.limit);
                Self::layout(*rect, *alpha, text, &font.font, style, &mut |sf, line, start, rgba| {
                    self.glyphs(sf, line, start, clip, rgba)
                });
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
        if (rw, rh) == (bmp.w as f32, bmp.h as f32) && rect[0].fract() == 0.0 && rect[1].fract() == 0.0 {
            // 1:1 on whole pixels: every sample falls on a texel centre, where the bilinear filter is that texel.
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = bmp.texel(x - rect[0] as i32, y - rect[1] as i32);
                    self.blend(x, y, p.map(|c| (c * a).round().clamp(0.0, 255.0) as u8));
                }
            }
            return;
        }
        // The bilinear taps of every sample, per column and per row (the same for all pixels of a column or row).
        let cols = taps(x0..x1, rect[0], sx, nx, bmp.w);
        let rows = taps(y0..y1, rect[1], sy, ny, bmp.h);
        let n = (nx * ny) as f32;
        for (y, ty) in (y0..y1).zip(rows.chunks_exact(ny as usize)) {
            for (x, tx) in (x0..x1).zip(cols.chunks_exact(nx as usize)) {
                let mut acc = [0f32; 4];
                for &(ya, yb, fy) in ty {
                    for &(xa, xb, fx) in tx {
                        let p = bmp.bilinear((xa, xb, fx), (ya, yb, fy));
                        for (s, c) in acc.iter_mut().zip(p) {
                            *s += c;
                        }
                    }
                }
                self.blend(x, y, acc.map(|c| (c / n * a).round().clamp(0.0, 255.0) as u8));
            }
        }
    }

    /// The scaled font the text of `rect` is set in: its size, shrunk for a single line that is too wide.
    fn scaled_for<'f>(rect: Rect, text: &str, font: &'f FontArc, st: &TextStyle) -> Sf<'f> {
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
        scaled(size)
    }

    /// Lay the text out: `run(font, line, pen start, colour)` for every line of every layer (shadow, then text), in
    /// drawing order.
    fn layout(rect: Rect, alpha: f32, text: &str, font: &FontArc, st: &TextStyle, run: &mut TextRun<'_>) {
        let width = rect[2] - rect[0];
        let sf = Self::scaled_for(rect, text, font, st);
        let lines = wrap(&sf, text, width, st.lines);
        let line_h = sf.height() + sf.line_gap();
        let top = rect[1] + ((rect[3] - rect[1]) - line_h * lines.len() as f32).max(0.0) / 2.0;
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
                run(&sf, line, (x + off[0], baseline + off[1]), rgba);
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
            // A glyph wholly outside the clip (with a pixel to spare) has nothing to draw.
            if b.max.x + 1.0 < clip[0] || b.min.x - 1.0 > clip[2] || b.max.y + 1.0 < clip[1] || b.min.y - 1.0 > clip[3]
            {
                continue;
            }
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

/// Pixel boxes of the glyphs of `line` set from `start` (what [`Canvas::glyphs`] may touch).
fn glyph_boxes(sf: &Sf<'_>, line: &str, start: (f32, f32)) -> Vec<Area> {
    let mut out = Vec::new();
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
        if let Some(o) = sf.font().outline_glyph(g) {
            let b = o.px_bounds();
            let (x0, y0) = (b.min.x as i32, b.min.y as i32);
            out.push((x0, y0, x0 + b.width() as i32, y0 + b.height() as i32));
        }
    }
    out
}

/// The two texels a bilinear sample falls between along one axis (clamped to the bitmap) and its weight.
type Tap = (i32, i32, f32);

/// For each pixel of `range` the `n` samples along one axis of an image placed at `origin`, `scale` texels per pixel,
/// on a bitmap `size` texels long.
fn taps(range: std::ops::Range<i32>, origin: f32, scale: f32, n: u32, size: u32) -> Vec<Tap> {
    let last = size as i32 - 1;
    let mut out = Vec::with_capacity(range.len() * n as usize);
    for p in range {
        for i in 0..n {
            let u = (p as f32 + (i as f32 + 0.5) / n as f32 - origin) * scale - 0.5;
            let u0 = u.floor();
            let t = u0 as i32;
            out.push((t.clamp(0, last), (t + 1).clamp(0, last), u - u0));
        }
    }
    out
}

impl Bmp {
    fn texel(&self, x: i32, y: i32) -> [f32; 4] {
        let (x, y) = (x.clamp(0, self.w as i32 - 1) as usize, y.clamp(0, self.h as i32 - 1) as usize);
        let i = (y * self.w as usize + x) * 4;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]].map(f32::from)
    }

    /// The filtered colour between texel columns `xa, xb` and rows `ya, yb` (see [`taps`]).
    fn bilinear(&self, (xa, xb, fx): Tap, (ya, yb, fy): Tap) -> [f32; 4] {
        let (a, b, c, d) = (self.texel(xa, ya), self.texel(xb, ya), self.texel(xa, yb), self.texel(xb, yb));
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
