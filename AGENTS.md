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
NDI → GPU → screen chain is real and verified, and there is a working GUI;
display output and Syphon/Spout are not built.

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
  unmapper-stagefile the stage XML format. roxmltree reads, quick-xml writes.
  unmapper-ndi       dlopen'd NDI. sys.rs is the raw FFI; lib.rs is the safe layer.
  unmapper-render    wgpu. panel.wgsl is shared by both views; blit.wgsl crops for
                     outputs; model.wgsl shades the set; model.rs reads glTF.
  unmapper-gui       the desktop app. state.rs is testable without a window; ui.rs is
                     widgets; outputs.rs owns the monitor windows.
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

- **The stage file is real XML, not JSON in a CDATA block.** `openstage` does the
  latter and is right to; its sections are tagged enums and HashMaps that would
  drift from the serde mapping. A stage is tree-shaped and the point of the format
  is that a person can read and edit it, so this one is a genuine mapping. The
  round-trip tests are what stop it drifting.

- **Numbers in the stage file use plain `{}` formatting.** Rust's `Display` for
  `f32` prints the shortest string that parses back to the same bits, *and* omits
  a trailing `.0`. An earlier version rounded to 4dp for tidiness and perturbed
  every panel rotation on each save.

- **`Gpu` owns the `wgpu::Instance`.** A surface belongs to the instance that
  created it; pairing it with a device from a second instance panics inside
  wgpu-core with "Surface does not exist", which reads like a lifetime bug and
  is not one.

- **In the GUI, egui texture deltas are applied before the surface is acquired.**
  egui sends a texture as one allocation then partial updates. `present()` can
  legitimately return early (surface timeout during startup); skipping that
  frame's deltas loses the allocation and the next partial update panics with
  "Tried to update a texture that has not been allocated yet".

- **The canvas is rendered once and everything else is a crop of it.** The
  viewport blits it with pan/zoom as a source rect; each output window blits its
  own region. This is not just an optimisation — it is what makes it impossible
  for two monitors to show different frames of the same wall.

- **Output blits sample NEAREST.** One canvas pixel is one LED, so interpolating
  between them is meaningless, and a linear filter would hide a region/monitor
  size mismatch behind a plausible blur. Blocky is the honest signal. The bind
  group layout declares the sampler non-filtering to match.

- **The backdrop is in the viewport scene and NOT the canvas scene.** That is the
  whole reason `build_viewport_scene` and `build_canvas_scene` are separate
  functions, and why the viewport renders itself instead of cropping the canvas
  as it briefly did. An editing aid reaching a monitor standing in for the wall
  would be a bug the operator only discovers in a venue. There is a test that
  renders both and asserts the canvas stays black.

- **glTF node transforms are baked into vertices at load.** A real export nests
  geometry under a hierarchy; walking it once and flattening gives one buffer and
  one draw. The cost is that the model cannot be re-articulated afterwards, which
  nothing wants to do. Do not "fix" this by keeping the tree.

- **glTF `NORMAL` is optional and plenty of CAD exports omit it.** Without the
  face-normal fallback in `append_primitive`, those files shade flat black — which
  looks like a broken loader rather than a missing attribute.

- **An NDI output costs a GPU readback every frame.** There is no way around
  that while the SDK takes CPU pixels, which is why NDI is opt-in per output
  rather than always running, and why the readback is the last thing in a frame.

- **New outputs default to windowed, not fullscreen.** A fullscreen window that
  opens on the wrong monitor before the region is right is hard to dismiss.
  Closing an output window disables that output rather than quitting the app.

- **WGSL `vec3<f32>` is 16-byte *aligned*.** A `vec3` padding field in a uniform
  block does not pack like a Rust `[f32; 3]` — it pushes everything after it and
  changes the block size. `Globals` uses three scalars for exactly this reason.

## 5. How to tell built from unbuilt

Built and verified against real hardware:
- Import, on 4 real Resolume files (`crates/unmapper-resolume/tests/fixtures/`).
- NDI receive, against a live NDI 6.3.2 sender — 1920x1080 RGBA, 50 fps, 0 drops.
- Both render paths, by GPU pixel readback (`crates/unmapper-render/tests/render.rs`).
- The whole chain, via the CLI, producing correct PNGs from a live sender.
- The GUI, launched with a stage, showing live NDI in the viewport at 50 fps —
  confirmed by screenshot, with the sources panel reporting the real format and
  rate and the status bar reporting no problems.
- **The backdrop and the set model**, by GPU readback: the backdrop appears in
  the viewport scene and provably not in the canvas scene, opacity fades it, it
  draws beneath the panels, and a named-but-unloaded image is simply skipped.
  glTF loading is checked against a fixture with a nested, scaled, translated
  node — a loader ignoring the hierarchy fails it — and the whole previz path was
  rendered to a PNG showing a set with live video on the walls.
- **Previz to an output**: an 800x450 previz camera view published over NDI at
  120 fps, zero drops, received by the probe. A previz output is rendered on its
  own (a camera view is not a crop of anything) into a target sized to whatever
  the largest previz output asked for, then blitted like any other.
- **NDI output**: UnMapper published a 960x1080 RGBA source at 50 fps, zero
  drops, received by an independent probe — so NDI in → GPU → NDI out is a
  closed loop.
- **Display output**: two output windows opened from one stage, each showing the
  correct half of the canvas (confirmed by screenshot — the right output showed
  the right half's colour bars), live, with the status bar reading "2/2 output(s)
  open" and the not-1:1 warning firing. Blit correctness is also covered by GPU
  readback tests, including that a full-canvas blit is byte-identical to the
  canvas.

**Partly verified:**
- The GUI's **Previz tab has never been clicked** — driving the click needed
  assistive access this machine does not grant. The previz *renderer* is covered
  by a GPU readback test and by `unmapper render --previz`, and the orbit camera
  is unit-tested, but the tab itself is unexercised. Same for drag-to-place,
  the file dialogs and the NDI rescan button: the logic under them is unit-tested
  in `state.rs`, the widgets are not.

**Not built.** Do not describe any of these as working:
- Syphon and Spout. Modelled in `OutputTarget`, validated as platform-specific,
  no implementation. There is no `unmapper-share` crate yet.
- Loading the 3D CAD model or the 2D backdrop image. `Model3d` and `Backdrop`
  exist in the show file and nothing reads them.
- Slice `orientation` (flip/rotate) — parsed, warned about, **not applied, on
  purpose**. Every real Resolume Advanced Output available here has
  `orientation="0"` on every rect, so the integer→transform mapping cannot be
  determined from evidence. Guessing it would make walls wrong in a *new* way
  rather than leaving a known gap. It needs one exported slice map with a flipped
  slice to pin down; do not implement it before that exists.
- Resolume's `Warper` (bezier mesh / homography) — detected, warned about, not reproduced.
- Anything on a real LED wall. No venue, no processor, no panel has ever seen this.

## 6. Releasing

`scripts/release-local.sh` is the release — there is no CI. Tests, clippy, a
universal macOS binary, and a `UnMapper.app`.

It ends with a check that `Contents/MacOS/UnMapper` is really the GUI, because it
once was not: **macOS filesystems are case-insensitive by default**, so copying
both `UnMapper` and `unmapper` into `Contents/MacOS` produces *one* file, and the
CLI silently replaced the GUI. The bundle then launched, printed usage to a log
nobody reads, and exited. The CLI is `unmapper-cli` in the bundle for this reason.

## 7. Build and test

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
