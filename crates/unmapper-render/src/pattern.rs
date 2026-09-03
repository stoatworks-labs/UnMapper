//! A built-in test pattern, for checking a rig's geometry with nothing plugged in.
//!
//! The point is not to look like a test card — `test-card` in this fleet already
//! does that properly, measured against RP 219. This is a *geometry* aid: every
//! feature exists so a mistake in the slice map or the panel layout is obvious at
//! a glance, on a wall, from the back of a room.

use unmapper_core::Size;

/// Generate an RGBA test pattern.
///
/// - A **grid** on a fixed pixel pitch, so a slice sampling the wrong region
///   shows a discontinuity in the lines rather than a plausible picture.
/// - **Corner markers**, so a slice that is flipped, rotated or off by a few
///   pixels is unmistakable — a symmetric pattern would hide exactly that.
/// - A **centre cross** and a bright **1px border**, so the edges of each source
///   are visible where panels butt together.
pub fn test_pattern(size: Size) -> Vec<u8> {
    let (w, h) = (size.width.max(1), size.height.max(1));
    let mut data = vec![0u8; (w as usize) * (h as usize) * 4];

    // A grid coarse enough to see from a distance but fine enough to localise an
    // error to a panel rather than a wall.
    const GRID: u32 = 64;
    let corner = (w.min(h) / 8).clamp(8, 160);

    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;

            let on_grid = x % GRID == 0 || y % GRID == 0;
            let on_border = x == 0 || y == 0 || x == w - 1 || y == h - 1;
            let on_cross =
                (x as i64 - (w / 2) as i64).abs() < 2 || (y as i64 - (h / 2) as i64).abs() < 2;

            // Each corner a different colour, so orientation is readable without
            // reading any text.
            let in_corner = x < corner && y < corner
                || x >= w - corner && y < corner
                || x < corner && y >= h - corner
                || x >= w - corner && y >= h - corner;

            let (r, g, b) = if in_corner {
                match (x < corner, y < corner) {
                    (true, true) => (220, 40, 40),    // top-left red
                    (false, true) => (40, 200, 40),   // top-right green
                    (true, false) => (40, 90, 220),   // bottom-left blue
                    (false, false) => (220, 200, 40), // bottom-right yellow
                }
            } else if on_border || on_cross {
                (255, 255, 255)
            } else if on_grid {
                (110, 110, 120)
            } else {
                (16, 16, 20)
            };

            data[i] = r;
            data[i + 1] = g;
            data[i + 2] = b;
            data[i + 3] = 255;
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(d: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [d[i], d[i + 1], d[i + 2], d[i + 3]]
    }

    #[test]
    fn the_four_corners_differ_so_a_flip_is_visible() {
        let (w, h) = (512, 512);
        let d = test_pattern(Size::new(w, h));
        let tl = px(&d, w, 4, 4);
        let tr = px(&d, w, w - 5, 4);
        let bl = px(&d, w, 4, h - 5);
        let br = px(&d, w, w - 5, h - 5);
        // A pattern symmetric in either axis could not reveal a flipped slice.
        assert_ne!(tl, tr, "horizontal flip would be invisible");
        assert_ne!(tl, bl, "vertical flip would be invisible");
        assert_ne!(tl, br, "180 rotation would be invisible");
    }

    #[test]
    fn it_is_fully_opaque_and_the_right_size() {
        let d = test_pattern(Size::new(64, 32));
        assert_eq!(d.len(), 64 * 32 * 4);
        assert!(d.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }

    #[test]
    fn a_degenerate_size_does_not_panic_or_produce_nothing() {
        assert_eq!(test_pattern(Size::new(0, 0)).len(), 4);
    }
}
