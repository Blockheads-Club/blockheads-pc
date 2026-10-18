//! Where the game's fixed-size surface sits in the window.
//!
//! The game picks its own render size (`VerdeActivity.setContentSize`, like an Android fixed-size surface) and
//! expects the window system to scale it to the screen. The window keeps an offscreen target of that size and draws
//! it into the drawable scaled uniformly and centred (letterboxed); this module is the arithmetic of that step.

/// A rectangle in drawable pixels, origin at the bottom-left (GL convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// The largest centred rectangle with the aspect of `content` that fits `drawable`. Degenerate sizes give an
/// empty viewport.
pub fn fit(content: (u32, u32), drawable: (u32, u32)) -> Viewport {
    let (cw, ch) = (u64::from(content.0), u64::from(content.1));
    let (dw, dh) = (u64::from(drawable.0), u64::from(drawable.1));
    if cw == 0 || ch == 0 || dw == 0 || dh == 0 {
        return Viewport { x: 0, y: 0, w: 0, h: 0 };
    }
    // Compare dw/cw with dh/ch without floating point: the width is the limit when dw * ch <= dh * cw.
    let (w, h) =
        if dw * ch <= dh * cw { (dw, (ch * dw).div_ceil(cw).min(dh)) } else { ((cw * dh).div_ceil(ch).min(dw), dh) };
    Viewport { x: ((dw - w) / 2) as i32, y: ((dh - h) / 2) as i32, w: w as i32, h: h as i32 }
}

/// Pack a size into one atomic word (0 = unset; both sides are non-zero when set).
pub fn pack(w: u32, h: u32) -> u64 {
    (u64::from(w) << 32) | u64::from(h)
}

/// Inverse of [`pack`]; `None` for an unset or degenerate size.
pub fn unpack(v: u64) -> Option<(u32, u32)> {
    let (w, h) = ((v >> 32) as u32, v as u32);
    (w != 0 && h != 0).then_some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_aspect_fills_the_drawable() {
        assert_eq!(fit((1024, 768), (1280, 960)), Viewport { x: 0, y: 0, w: 1280, h: 960 });
        assert_eq!(fit((1024, 768), (1024, 768)), Viewport { x: 0, y: 0, w: 1024, h: 768 });
        assert_eq!(fit((1024, 768), (2048, 1536)), Viewport { x: 0, y: 0, w: 2048, h: 1536 });
    }

    #[test]
    fn other_aspects_are_letterboxed_and_centred() {
        // A wide window: pillarbox.
        assert_eq!(fit((1024, 768), (1920, 768)), Viewport { x: 448, y: 0, w: 1024, h: 768 });
        // A tall window: bars above and below.
        assert_eq!(fit((1024, 768), (1024, 1000)), Viewport { x: 0, y: 116, w: 1024, h: 768 });
        // Upscaling keeps the aspect.
        assert_eq!(fit((1334, 750), (2000, 2000)), Viewport { x: 0, y: 437, w: 2000, h: 1125 });
        // Never larger than the drawable even after rounding up.
        let v = fit((3, 7), (10, 10));
        assert!(v.w <= 10 && v.h <= 10 && v.x >= 0 && v.y >= 0);
    }

    #[test]
    fn degenerate_sizes_give_an_empty_viewport() {
        let empty = Viewport { x: 0, y: 0, w: 0, h: 0 };
        for (c, d) in [((0, 5), (5, 5)), ((5, 0), (5, 5)), ((5, 5), (0, 5)), ((5, 5), (5, 0))] {
            assert_eq!(fit(c, d), empty);
        }
    }

    #[test]
    fn sizes_pack_into_one_word() {
        assert_eq!(unpack(0), None);
        assert_eq!(unpack(pack(0, 5)), None);
        assert_eq!(unpack(pack(5, 0)), None);
        assert_eq!(unpack(pack(1024, 768)), Some((1024, 768)));
    }
}
