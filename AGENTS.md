# AGENTS.md — bringing an LLM up to speed on UnMapper

Orientation for an AI assistant (or a new human) picking this project up cold.
`CLAUDE.md` is the short command reference; this file is the mental model, the
invariants, and how to tell finished work from scaffolding.

---

## 1. What this is

UnMapper reads a **Resolume Arena Advanced Output** (the slice map that drives a
real LED wall), receives Resolume's outputs over **NDI**, and composites them
onto a virtual reconstruction of the rig — either flat and pixel-exact for
driving a bank of ordinary monitors as a stand-in wall, or in 3D through a
previz camera.

Public, MIT.

**It is early.** See the status table in `README.md` and §5 below. The import →
NDI → GPU → image chain is real and verified; the GUI, display output, and
Syphon/Spout are not built.

## 2. The five coordinate spaces

This is the thing to internalise before changing anything. They are documented in
`crates/unmapper-core/src/lib.rs` and repeated in the README.

Composition px · Screen raster px · Virtual raster px · Stage metres · Display px

**The invariant that matters most:** a slice carries *two* quads. `input` is
where it reads from in composition space; `output` is where it writes to in its
screen's raster space. Which one UnMapper samples with is decided by
`SourceSpace`, because it depends on what the NDI sender is actually sending —
the whole composition, or one already-sliced output per screen. Getting this
backwards produces a wall that looks plausible and is wrong.

## 3. Layout

```
crates/
  unmapper-core      domain model. No GPU, no I/O, no windowing. Pure and heavily tested.
  unmapper-resolume  Advanced Output reader. Tested against 4 real files in tests/fixtures/.
  unmapper-ndi       dlopen'd NDI. sys.rs is the raw FFI; lib.rs is the safe layer.
  unmapper-render    wgpu. panel.wgsl is shared by both views.
  unmapper-app       the `unmapper` binary (CLI).
  diag               vendored from the fleet — do not edit here, it is a copy.
```

## 4. Decisions that look wrong until you know why

- **Quads, not rects, everywhere.** The sibling reader in `test-card` collapses
  each slice to a bounding box, which is right for *drawing outlines* and wrong
  here, because these coordinates feed a sampler. A corner-pinned slice
  collapsed to its bbox reads the wrong texels. `Quad` is the primitive.

- **`Quad::projective_weights` and the `uvq` varying.** Interpolating u and v
  linearly across two triangles is a *bilinear* map; the correct map for a
  corner-pinned slice is *projective*, and the two disagree along the shared
  diagonal. The shader carries `(u*q, v*q, q)` and divides. An unwarped rect
  gets uniform weights, so the ordinary case is unaffected — which is why this
  is applied unconditionally rather than behind a branch.

- **wgpu is pinned to 29, not the newest.** `egui-wgpu` 0.35 depends on wgpu 29.
  Two wgpu versions in one tree compile fine and then refuse to share a `Device`,
  because `wgpu::Device` from 29 and from 30 are different types. If you bump
  egui, re-check with `cargo tree | grep -oE '(^|[^-])wgpu v[0-9.]+'` and make
  sure exactly one version comes back.

- **NDI is `dlopen`'d, never linked.** Licensing, not convenience — see the
  header of `crates/unmapper-ndi/src/sys.rs`. The canonical fleet write-up is
  `weblinked/docs/06-ndi-distribution.md`. Do not add a bindgen NDI crate.

- **The `#[repr(C)]` structs in `unmapper-ndi/src/sys.rs` are load-bearing.**
  Their layout tests are the only thing between a field-order typo and silent
  memory corruption. They were checked against the real vendored SDK headers.
  Do not change a struct without running them.

- **The render target is `Rgba8Unorm`, not sRGB.** The pixels are already
  whatever Resolume sent; re-encoding would shift every colour on the wall.

- **A re-imported slice map never deletes a panel.** `Show::reapply_slice_map`
  matches on `slice_id`, updates in place, and *disables* panels whose slice
  vanished. Deleting someone's hour of placement work because a slice was
  renamed would be unforgivable.

- **A live frame's size beats the slice map's `expected`.** The sender is the
  authority on what it is sending. A mismatch is *warned about* (it means the
  wall will be wrong) but the live size is what gets used.

## 5. How to tell built from unbuilt

Built and verified against real hardware:
- Import, on 4 real Resolume files (`crates/unmapper-resolume/tests/fixtures/`).
- NDI receive, against a live NDI 6.3.2 sender — 1920x1080 RGBA, 50 fps, 0 drops.
- Both render paths, by GPU pixel readback (`crates/unmapper-render/tests/render.rs`).
- The whole chain, via the CLI, producing correct PNGs from a live sender.

**Not built.** Do not describe any of these as working:
- Any GUI at all.
- Output to connected displays. `OutputTarget::Display` is modelled and never consumed.
- Syphon and Spout. Modelled in `OutputTarget`, validated as platform-specific,
  no implementation. There is no `unmapper-share` crate yet.
- Loading the 3D CAD model or the 2D backdrop image. `Model3d` and `Backdrop`
  exist in the show file and nothing reads them.
- NDI *output*. `unmapper_ndi::Sender` exists and is untested against a receiver.
- Slice `orientation` (flip/rotate) — parsed, warned about, not applied.
- Resolume's `Warper` (bezier mesh / homography) — detected, warned about, not reproduced.
- Anything on a real LED wall. No venue, no processor, no panel has ever seen this.

## 6. Build and test

```bash
cargo test                                  # everything
cargo test -p unmapper-render               # needs a working GPU adapter
cargo clippy --all-targets -- -D warnings
cargo run -p unmapper-ndi --example ndi_probe
```

The render tests are headless — no window, no surface — so they work over SSH.
They are the only check that the shader, the vertex transform, the UV mapping and
the y-flip all agree; geometry unit tests cannot catch a flipped canvas.

**Test fixtures avoid a trap:** the sampler filters linearly, so a small source
blends neighbouring texels into every assertion. The fixtures use large flat
quadrants so the sample point sits well inside one.
