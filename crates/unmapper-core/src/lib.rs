//! UnMapper's domain model — no GPU, no I/O, no windowing.
//!
//! # The coordinate spaces, once, in one place
//!
//! Five spaces, and naming them is most of the battle:
//!
//! | Space | Units | Holds |
//! |---|---|---|
//! | **Composition** | px | Resolume's composition raster. A slice's `input` quad. |
//! | **Screen raster** | px | One Resolume output. A slice's `output` quad. |
//! | **Virtual raster** | px | UnMapper's emulation canvas — the whole rig recreated flat. A panel's `layout`. |
//! | **Stage** | metres | The physical set, Y up. A panel's `placement`. |
//! | **Display** | px | An actual connected monitor. An output's `region` crops the virtual raster into one. |
//!
//! Frames arrive in composition *or* screen-raster space — see
//! [`show::SourceSpace`] — and are painted onto panels, which exist in the virtual
//! raster and in the stage at the same time. The two renderers then read whichever
//! of those two they need.

pub mod geom;
pub mod show;
pub mod slicemap;
pub mod stage;

pub use geom::{Quad, Rect, Vec2, Vec3};
pub use show::{
    Binding, Output, OutputTarget, OutputView, Problem, ReapplyReport, Severity, Show, ShowError,
    Source, SourceKind, SourceSpace, SHOW_FORMAT,
};
pub use slicemap::{RasterSource, Screen, Size, Slice, SliceMap};
pub use stage::{Backdrop, Camera, Model3d, Panel, Placement3d, StageGeometry, DEFAULT_PITCH_MM};
