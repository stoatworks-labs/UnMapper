# UnMapper

Recreates a physical LED rig from a Resolume Advanced Output, receives Resolume's
outputs over NDI, and composites them onto that reconstruction — flat and
pixel-exact for driving ordinary monitors as a stand-in wall, or in 3D previz.

**Read `AGENTS.md` before changing anything.** It carries the five coordinate
spaces, the decisions that look wrong until explained, and an explicit list of
what is *not* built. Public repo, MIT.

## Commands

- Build / test: `cargo build` · `cargo test`
- Lint: `cargo clippy --all-targets -- -D warnings`
- Run the GUI: `cargo run -p unmapper-gui [-- rig.unmapper.xml]`
- List NDI senders: `cargo run -p unmapper-app -- sources`
- Import a slice map: `cargo run -p unmapper-app -- import Advanced.xml -o rig.unmapper.xml`
- Bind a source: `cargo run -p unmapper-app -- bind rig.unmapper.xml --source 0 --ndi "NAME"`
- Check a show: `cargo run -p unmapper-app -- check rig.unmapper.xml`
- Render: `cargo run -p unmapper-app -- render rig.unmapper.xml -o wall.png [--previz]`
- Release build: `scripts/release-local.sh [--fast]` → `dist-release/UnMapper.app`
- NDI diagnostic: `cargo run -p unmapper-ndi --example ndi_probe [-- "SOURCE NAME"]`

## Layout (crates/)

- `unmapper-core` — domain model, no GPU/IO/windowing
- `unmapper-resolume` — Advanced Output reader
- `unmapper-stagefile` — the stage XML format
- `unmapper-ndi` — NDI, `dlopen`'d at run time
- `unmapper-render` — wgpu, both views, shared `panel.wgsl`
- `unmapper-gui` — the desktop app (`state.rs` is testable without a window,
  `outputs.rs` owns the monitor windows)
- `unmapper-app` — the `unmapper` CLI
- `diag` — vendored fleet diagnostics, a copy; don't edit here

## Traps

- **wgpu is pinned to 29** to match `egui-wgpu` 0.35. Two wgpu versions in one
  tree compile and then can't share a `Device`. Verify with
  `cargo tree | grep -oE '(^|[^-])wgpu v[0-9.]+'`.
- **NDI is loaded, never linked** — licensing, see `unmapper-ndi/src/sys.rs`.
  Don't add a bindgen NDI crate.
- **The `#[repr(C)]` NDI structs are load-bearing**; their layout tests guard
  against silent memory corruption.
- **Render target is `Rgba8Unorm`, not sRGB** — don't add a colour conversion.
- **Quads, not bounding boxes** — these coordinates feed a sampler.
- Render tests need a working GPU adapter, but no window.
- **`Gpu` owns the `wgpu::Instance`** — a surface from a *different* instance
  panics with "Surface does not exist".
- **WGSL `vec3<f32>` is 16-byte aligned** and will not pack like a Rust `[f32; 3]`
  in a uniform block. `Globals` pads with scalars.
- **egui texture deltas must be applied even when a frame is not presented**, or
  a later partial update panics.
- **Output blits are NEAREST on purpose** — a linear filter would hide a
  region/monitor size mismatch.
- **macOS filesystems are case-insensitive** — a bundle cannot hold both
  `UnMapper` and `unmapper`; the second copy silently replaces the first.
- **The backdrop belongs to the viewport scene only** — it must never reach an
  output. `build_viewport_scene` vs `build_canvas_scene` exist for this.

## Status

Import → NDI → GPU → screen → display outputs is built and verified end to end on
real hardware, including a working GUI showing live NDI, a 2D backdrop mockup and
a 3D set model. Previz reaches an output too, as a window or an NDI source
(`OutputView::Previz` in `unmapper-gui/src/outputs.rs`, round-tripped by
`unmapper-stagefile`). **No Syphon/Spout, and nothing has ever run on a real LED
wall.** Several GUI widgets are unexercised — see `AGENTS.md` §5.
