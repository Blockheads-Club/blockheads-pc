//! Window size and DPI scale.

/// Size of the window in logical pixels and of its drawable in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub logical: (u32, u32),
    pub drawable: (u32, u32),
}

impl Metrics {
    pub fn new(logical: (u32, u32), drawable: (u32, u32)) -> Metrics {
        Metrics { logical, drawable }
    }

    /// Content scale (2.0 on a 200 % display). 1.0 for an empty window.
    pub fn scale(&self) -> f32 {
        if self.logical.0 == 0 {
            1.0
        } else {
            self.drawable.0 as f32 / self.logical.0 as f32
        }
    }

    /// Logical window coordinates to framebuffer pixels.
    pub fn to_drawable(&self, x: f32, y: f32) -> (f32, f32) {
        let s = self.scale();
        (x * s, y * s)
    }

    /// Framebuffer pixels to logical window coordinates.
    pub fn to_logical(&self, x: f32, y: f32) -> (f32, f32) {
        let s = self.scale();
        (x / s, y / s)
    }

    /// Width / height of the drawable (1.0 for an empty window).
    pub fn aspect(&self) -> f32 {
        if self.drawable.1 == 0 {
            1.0
        } else {
            self.drawable.0 as f32 / self.drawable.1 as f32
        }
    }
}
