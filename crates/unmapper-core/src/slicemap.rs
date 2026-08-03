//! The slice map as Resolume describes it.
//!
//! This is the *imported* shape — a faithful reading of an Advanced Output, before
//! UnMapper has decided anything about where the pixels go. The mapping onto a
//! virtual stage lives in [`crate::show`]; keeping the two apart means re-importing
//! an updated slice map does not throw away the operator's placement work.
//!
//! # The two coordinate spaces a slice lives in
//!
//! Every slice carries two quads, and confusing them is the easiest mistake to make:
//!
//! - [`Slice::input`] is in **composition space** — where the slice *takes* its
//!   pixels from, inside Resolume's composition raster.
//! - [`Slice::output`] is in **screen raster space** — where the slice *puts* those
//!   pixels, inside that one screen's output raster.
//!
//! Which one UnMapper samples with depends entirely on what the NDI sender is
//! sending, which is why [`crate::show::SourceSpace`] exists.

use serde::{Deserialize, Serialize};

use crate::geom::Quad;
use crate::warp::WarpMesh;

/// A pixel size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub fn as_vec2(&self) -> glam::Vec2 {
        glam::Vec2::new(self.width as f32, self.height as f32)
    }
}

/// How much to trust a screen's raster dimensions.
///
/// Resolume only writes explicit dimensions for *virtual* output devices. A real
/// display or a capture card is recorded with a machine-specific `idHash` and no
/// size at all, so for those the raster has to be inferred — and the inference is
/// wrong (too small) whenever the slices do not reach the edges of the raster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RasterSource {
    /// Read from an `OutputDeviceVirtual`'s own width/height. Trustworthy.
    Declared,
    /// Inferred from how far the slices extend. A guess — see the type docs.
    SliceBounds,
    /// Nothing to go on; fell back to the composition size. Needs setting by hand.
    Fallback,
}

impl RasterSource {
    /// Whether the operator should be asked to confirm this raster.
    pub fn needs_confirmation(&self) -> bool {
        !matches!(self, RasterSource::Declared)
    }
}

/// One slice of an Advanced Output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slice {
    pub id: String,
    pub name: String,
    /// Where the slice reads from, in composition space.
    pub input: Quad,
    /// Where the slice writes to, in this screen's raster space.
    pub output: Quad,
    /// Resolume's own enable flag for the slice.
    pub enabled: bool,
    /// Resolume's `orientation` attribute — a flip/rotate applied on top of the
    /// corner positions. **Not yet applied by the renderer**; a non-zero value
    /// raises an import warning rather than being silently dropped.
    pub orientation: i32,
    /// The slice's warp lattice, in this screen's raster space — i.e. the same
    /// space as [`Slice::output`].
    ///
    /// `None` when the slice's warper was the untouched one Arena writes for
    /// every slice, so the common case costs nothing and renders down the
    /// single-quad path. See [`crate::warp`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warp: Option<WarpMesh>,
}

impl Slice {
    /// Whether either quad has been rotated or corner-pinned.
    ///
    /// Says nothing about the warp lattice — a slice can have square corners and
    /// a folded interior. Use [`Slice::warp`] for that.
    pub fn is_warped(&self) -> bool {
        !self.input.is_axis_aligned(0.5) || !self.output.is_axis_aligned(0.5)
    }
}

/// One screen — in Resolume terms one output device, and in the real world one
/// feed to an LED processor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    pub id: String,
    pub name: String,
    pub raster: Size,
    pub raster_source: RasterSource,
    /// The output device's label, when the file named one.
    pub device: Option<String>,
    pub slices: Vec<Slice>,
    /// Human-readable remarks raised while reading this screen.
    pub notes: Vec<String>,
}

/// A whole imported Advanced Output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SliceMap {
    pub project_name: String,
    /// Resolume's composition raster, when the file recorded it.
    pub composition: Option<Size>,
    pub screens: Vec<Screen>,
    /// Remarks about the import as a whole, for showing to the operator.
    pub warnings: Vec<String>,
}

impl SliceMap {
    pub fn slice_count(&self) -> usize {
        self.screens.iter().map(|s| s.slices.len()).sum()
    }

    /// Every slice, paired with the screen it came from.
    pub fn slices(&self) -> impl Iterator<Item = (&Screen, &Slice)> {
        self.screens
            .iter()
            .flat_map(|screen| screen.slices.iter().map(move |slice| (screen, slice)))
    }

    pub fn screen(&self, id: &str) -> Option<&Screen> {
        self.screens.iter().find(|s| s.id == id)
    }

    /// Total LED pixel count across every slice, using each slice's output quad.
    ///
    /// Uses bounding boxes, so it is an over-estimate for warped slices. It is a
    /// sanity figure for the UI ("does this look like the wall I think it is?"),
    /// not a processor capacity calculation.
    pub fn approximate_pixel_count(&self) -> u64 {
        self.slices()
            .map(|(_, s)| {
                let b = s.output.bounds();
                (b.width.max(0.0) as f64 * b.height.max(0.0) as f64) as u64
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rect;

    fn slice(id: &str, out: Rect) -> Slice {
        Slice {
            id: id.into(),
            name: id.into(),
            input: Quad::from_rect(out),
            output: Quad::from_rect(out),
            enabled: true,
            orientation: 0,
            warp: None,
        }
    }

    #[test]
    fn pixel_count_sums_every_slice() {
        let map = SliceMap {
            project_name: "t".into(),
            composition: Some(Size::new(1920, 1080)),
            screens: vec![Screen {
                id: "s".into(),
                name: "s".into(),
                raster: Size::new(1920, 1080),
                raster_source: RasterSource::Declared,
                device: None,
                slices: vec![
                    slice("a", Rect::new(0.0, 0.0, 960.0, 1080.0)),
                    slice("b", Rect::new(960.0, 0.0, 960.0, 1080.0)),
                ],
                notes: vec![],
            }],
            warnings: vec![],
        };
        assert_eq!(map.slice_count(), 2);
        assert_eq!(map.approximate_pixel_count(), 1920 * 1080);
    }

    #[test]
    fn declared_raster_is_the_only_one_not_needing_confirmation() {
        assert!(!RasterSource::Declared.needs_confirmation());
        assert!(RasterSource::SliceBounds.needs_confirmation());
        assert!(RasterSource::Fallback.needs_confirmation());
    }
}
