//! The game's tile grid on screen, for snapping the controller cursor to tiles.
//!
//! The game draws its world with a perspective camera looking straight at the tile plane: world units are tiles, the
//! plane is `z = 0`, and it keeps the matrices it maps taps with (`World.tapModelviewMatrix`,
//! `tapProjectionMatrix`, `tapViewport`). The plane-to-screen part of those matrices is a 2D projective map
//! (a homography); [`Grid`] holds it and its inverse, in content pixels (the unit of touches).

/// A point in content pixels.
pub type Pt = (f32, f32);

/// A tile: whole world coordinates (the tile spans `x..x + 1`, `y..y + 1`).
pub type Tile = (i32, i32);

/// Column-major 4x4 matrix (OpenGL / GLKit order).
pub type Mat4 = [f32; 16];

/// The tile plane `z = 0` as it appears on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    /// World (tile) coordinates to content pixels, row-major 3x3 on `(x, y, 1)`.
    to_screen: [f32; 9],
    /// Content pixels to world coordinates.
    to_world: [f32; 9],
}

fn apply(m: &[f32; 9], (x, y): Pt) -> Option<Pt> {
    let w = m[6] * x + m[7] * y + m[8];
    if w.abs() < 1e-9 || !w.is_finite() {
        return None;
    }
    Some(((m[0] * x + m[1] * y + m[2]) / w, (m[3] * x + m[4] * y + m[5]) / w))
}

fn inverse(m: &[f32; 9]) -> Option<[f32; 9]> {
    let [a, b, c, d, e, f, g, h, i] = m.map(f64::from);
    let (ca, cb, cc) = (e * i - f * h, -(d * i - f * g), d * h - e * g);
    let det = a * ca + b * cb + c * cc;
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    let inv =
        [ca, -(b * i - c * h), b * f - c * e, cb, a * i - c * g, -(a * f - c * d), cc, -(a * h - b * g), a * e - b * d];
    Some(inv.map(|v| (v / det) as f32))
}

impl Grid {
    /// The grid from the game's tap matrices: `modelview` and `projection` (column-major), `viewport`
    /// (`x, y, w, h` in the game's points, origin bottom-left as in GL) and the content size the touches use.
    /// `None` when the matrices do not describe a usable view.
    pub fn from_gl(modelview: &Mat4, projection: &Mat4, viewport: [i32; 4], content: (f32, f32)) -> Option<Grid> {
        let [vx, vy, vw, vh] = viewport.map(|v| v as f32);
        if vw <= 0.0 || vh <= 0.0 || content.0 <= 0.0 || content.1 <= 0.0 {
            return None;
        }
        // clip = P * MV * (x, y, 0, 1): only columns 0, 1 and 3 of the product matter.
        let mvp = |r: usize, c: usize| (0..4).map(|k| projection[k * 4 + r] * modelview[c * 4 + k]).sum::<f32>();
        let col = [0usize, 1, 3];
        let row = |r: usize| col.map(|c| mvp(r, c));
        let (cx, cy, cw) = (row(0), row(1), row(3));
        // Window (GL, bottom-left origin): wx = vx + (ndc_x + 1) / 2 * vw, wy = vy + (ndc_y + 1) / 2 * vh.
        // Content pixels (top-left origin), scaled from viewport points: px = wx * sx, py = (vh - wy) * sy.
        let (sx, sy) = (content.0 / vw, content.1 / vh);
        let mut m = [0f32; 9];
        for k in 0..3 {
            let x = vx * cw[k] + (cx[k] + cw[k]) * vw / 2.0;
            let y = (vh - vy) * cw[k] - (cy[k] + cw[k]) * vh / 2.0;
            m[k] = x * sx;
            m[3 + k] = y * sy;
            m[6 + k] = cw[k];
        }
        let inv = inverse(&m)?;
        Some(Grid { to_screen: m, to_world: inv })
    }

    /// Where the world point `w` is on screen (content pixels).
    pub fn screen(&self, w: Pt) -> Option<Pt> {
        apply(&self.to_screen, w)
    }

    /// The world point (tile units) under the content pixel `p`.
    pub fn world(&self, p: Pt) -> Option<Pt> {
        apply(&self.to_world, p)
    }

    /// The centre of the tile under `p`, or `p` itself when that cannot be worked out.
    pub fn snap(&self, p: Pt) -> Pt {
        self.world(p).and_then(|(x, y)| self.screen((x.floor() + 0.5, y.floor() + 0.5))).unwrap_or(p)
    }

    /// The centre of the tile `(dx, dy)` tiles from the one under `p` (`dy` positive goes down the screen).
    pub fn step(&self, p: Pt, dx: i32, dy: i32) -> Pt {
        let up = self.screen_up();
        self.world(p)
            .and_then(|(x, y)| {
                self.screen((x.floor() + 0.5 + dx as f32, y.floor() + 0.5 + if up { -dy } else { dy } as f32))
            })
            .unwrap_or(p)
    }

    /// The tile under the content pixel `p`.
    pub fn tile(&self, p: Pt) -> Option<Tile> {
        self.world(p).filter(|(x, y)| x.is_finite() && y.is_finite()).map(|(x, y)| (x.floor() as i32, y.floor() as i32))
    }

    /// Where the centre of tile `t` is on screen.
    pub fn centre(&self, t: Tile) -> Option<Pt> {
        self.screen((t.0 as f32 + 0.5, t.1 as f32 + 0.5))
    }

    /// The tile `(dx, dy)` tiles from `t` on screen (`dy` positive goes down the screen).
    pub fn neighbour(&self, t: Tile, dx: i32, dy: i32) -> Tile {
        (t.0.saturating_add(dx), t.1.saturating_add(if self.screen_up() { -dy } else { dy }))
    }

    /// Size of one tile on screen around `p` (content pixels), 0 when unknown.
    pub fn tile_size(&self, p: Pt) -> f32 {
        let ends = self.world(p).and_then(|(x, y)| Some((self.screen((x, y))?, self.screen((x + 1.0, y))?)));
        ends.map_or(0.0, |(a, b)| ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt())
    }

    /// True when world `y` grows towards the top of the screen.
    fn screen_up(&self) -> bool {
        let ends = self.screen((0.0, 0.0)).zip(self.screen((0.0, 1.0)));
        ends.is_none_or(|(a, b)| b.1 < a.1)
    }
}
