# Notes

Working notes for this repo: status, decisions, and the traps that have actually bitten.
Migrated out of Claude Code's memory on 2026-08-24, so they are written in the first
person and dated by when each thing was learned — that date is usually the useful part.

Cross-cutting notes that are not specific to this repo live in
[fleet-notes](https://github.com/stoatworks-labs/fleet-notes).

## unmapper

*UnMapper — recreates an LED rig from a Resolume slice map and plays NDI onto it; PUBLIC MIT, Rust+wgpu, CLI only so far*

**UnMapper** (`~/Projects/UnMapper`, PUBLIC MIT, started 2026-08-01) reads a
Resolume Arena **Advanced Output** slice map, receives Resolume's outputs over
**NDI**, and composites them onto a virtual reconstruction of a physical LED rig.
Two views of one stage, both built: **emulation** (flat, pixel-exact, one canvas
pixel per LED — for driving a bank of monitors as a stand-in wall) and **previz**
(the same panels at 3D poses, through a camera).

**Repo actually created 2026-08-02.** Before that this file said PUBLIC while
no GitHub repo existed at all — no remote, 8 commits living only on the Mac,
unbacked-up. Now really at github.com/stoatworks-labs/UnMapper, branch renamed
master -> main to match the fleet. Lesson: a "PUBLIC" line in a project memory
is a claim to check with `gh repo view`, not a fact — two other memories had
the same drift (animATEM said PRIVATE while public).

Originally named "virtualLED"; renamed to UnMapper on day one.

Rust workspace: `unmapper-core` (domain, no GPU/IO), `unmapper-resolume`
(importer), `unmapper-stagefile` (the stage XML format), `unmapper-ndi` (dlopen'd
NDI), `unmapper-render` (wgpu), `unmapper-gui` (desktop app), `unmapper-app` (the
`unmapper` CLI), plus the vendored `diag`.

Stages save as **real XML** (`.unmapper.xml`), not JSON-in-CDATA the way
[openstage](https://github.com/stoatworks-labs/openstage/blob/main/docs/NOTES.md) (`openstage`) does it — a stage is tree-shaped and meant to be hand-edited.
Lossless round trip, byte-identical on re-save.

**Verified on real hardware, not just compiled:** import against 4 real Arena
7.27 files; NDI receive from a live 6.3.2 sender at 1920x1080 RGBA / 50 fps / 0
drops; both render paths by GPU pixel readback; the whole chain end to end via
the CLI; the **GUI running with live NDI in its viewport**; and **display output**
— two output windows each showing the correct half of the canvas, live. 90 tests,
clippy clean.

The canvas is rendered **once** per frame and everything else (viewport, every
output window) is a *crop* of it — so two monitors structurally cannot show
different frames. Output blits sample **NEAREST** on purpose: one canvas pixel is
one LED, and a linear filter would hide a region/monitor size mismatch behind a
plausible blur.

**Partly verified:** the GUI's Previz tab, drag-to-place, file dialogs and rescan
button have never been *clicked* — osascript has no assistive access on this Mac.
The logic under them is unit-tested in `unmapper-gui/src/state.rs`; the widgets
are not. See **screenshot capture** (working-practice note, kept in Claude memory).

**Geometry, both built:** a **2D backdrop** mockup (viewport only — it is an
editing aid and must never reach an output; `build_viewport_scene` vs
`build_canvas_scene` exist for exactly this) and a **3D set model** from
glTF/GLB (geometry only; nested node transforms baked at load; missing NORMALs
filled per face, else CAD exports shade flat black).

An output picks a **target** (Display or NDI) and a **view** (Emulation crop or
Previz camera) independently; all four combinations work.

**The main feature, in the author's own words (2026-08-03):** let a user
visualise their Resolume show *with its real geometry*, instead of the packed
layout Advanced Output and LED processors want to see. Previs only — it never
talks to live hardware. Built in two phases, **both done 2026-08-03**: the Resolume
warp lattice, then non-planar panel surfaces (`Surface::Flat` / `Arc` /
`Lattice`) — the latter is where 3D topology comes from, since a 2D lattice can
only deform in-plane. **No GUI for editing a surface yet** — hand-write it into
the stage XML. The two halves meet: the lattice removes Resolume's
pre-distortion, the surface puts back the shape it was compensating for.

**Two traps found doing it.** `Quad::projective_weights` is only valid
quad→texture (what the shader does); reusing it to get a *position* from (u,v)
is **not projective at all** and lands whole pixels out on a keystone —
`Quad::project` solves the real homography instead. And a curved `Surface` must
**never** touch the emulation canvas (always flat, always 1x1), or a stand-in
monitor stops being pixel-exact.

**Warp lattice: built 2026-08-03.** `BezierWarper` → `unmapper_core::WarpMesh`,
rendered one quad per cell. The direction matters and is counter-intuitive: a
warp is *pre-distortion* for a non-flat surface, and the processor still patches
a plain rect, so sampling the output rect shows what goes down the wire, **not
what the audience sees** — UnMapper samples *through* the lattice to undo it.
Evidence gap: **every Advanced Output on this Mac has an untouched lattice on
every slice** (same problem as `orientation`), so the warped fixture is
hand-authored and labelled synthetic. Space/order are pinned against real files;
what Arena writes for a *dragged* point is not.

**NOT built — do not describe as working:** Syphon/Spout (Syphon.framework is not
installed system-wide here — only bundled inside OBS/TouchDesigner/QLab etc — so
it can be neither built nor verified; **NDI output is built and verified
instead**, 960x1080 @ 50fps received by an independent probe). Non-planar
panels. Slice `orientation`, a non-identity `Homography`, and any `Point Mode`
other than `PM_LINEAR` are parsed and warned about but not applied. Nothing has
ever run on a real LED wall.

Release: `scripts/release-local.sh` (no CI) → universal macOS binary +
`dist-release/UnMapper.app`. macOS only on purpose — never run on Windows/Linux.

Reuses the fleet heavily — see **unmapper fleet reuse** (below). Related:
[weblinked](https://github.com/stoatworks-labs/weblinked/blob/main/docs/NOTES.md) (`weblinked`) (NDI licensing doc + the live test sender),
[openstage](https://github.com/stoatworks-labs/openstage/blob/main/docs/NOTES.md) (`openstage`) (the NDI loader and wgpu precedent),
[test card](https://github.com/stoatworks-labs/test-card/blob/main/docs/NOTES.md) (`test-card`), [blend calc](https://github.com/stoatworks-labs/blend-calc/blob/main/docs/NOTES.md) (`blend-calc`), [pixel peeker](https://github.com/stoatworks-labs/pixel-peeker/blob/main/docs/NOTES.md) (`pixel-peeker`).

## unmapper fleet reuse

*Where UnMapper's pieces came from in the fleet, and the two traps found building it (wgpu/egui version skew, linear-filter test fixtures)*

Building **unmapper** (below) cold, four fleet projects already had the hard
parts. Check these before writing anything new in this problem space:

- **`test-card/src/import/resolume.ts`** — a working Resolume Advanced Output
  reader off real Arena 7.27 files. Ported to Rust in `unmapper-resolume`, with
  one deliberate change: test-card collapses each slice to a **bounding box**
  (right for drawing outlines), UnMapper keeps all **four corners** (required,
  because these coordinates feed a sampler). Its fixtures were copied over and
  include files written by `blend-calc` and `pixel-peeker`, so they also prove
  the fleet's own round trip.
- **`openstage/crates/native-node/src/ndi_sys.rs`** — Rust `dlopen` NDI loader
  with Find + Send and `#[repr(C)]` layout tests. Had **no Recv**; UnMapper adds
  it. Verified my additions against the real vendored headers in
  `weblinked/third_party/ndi/include`.
- **`weblinked/docs/06-ndi-distribution.md`** — the canonical licensing reason
  NDI is loaded and never linked.
- **`openstage/crates/native-node`** — the wgpu precedent generally.

## Traps worth remembering

**wgpu/egui version skew.** `cargo add wgpu` pulls the newest (30), but
`egui-wgpu` 0.35 depends on **wgpu 29**. Two wgpu versions in one tree compile
fine and then refuse to share a `Device`, because `wgpu::Device` from 29 and 30
are different types. Always pin wgpu to whatever egui-wgpu wants and verify:
`cargo tree | grep -oE '(^|[^-])wgpu v[0-9.]+'` must return exactly one version.
(A naive `grep wgpu` also matches the `egui-wgpu` suffix and looks like a
conflict when there isn't one.)

**Linear filtering ruins small GPU test fixtures.** A 2x2 texture is all texel
boundary, so a readback assertion comes out a few percent off every pure colour
(got `[230,13,13]` expecting `[255,0,0]`) and looks like a real bug. Use a source
with large flat regions so the sample point sits well inside one.

**Three that only showed up on running the GUI:**

- A `wgpu::Surface` belongs to the `Instance` that created it. Pairing it with a
  device from a *second* instance panics in wgpu-core with **"Surface does not
  exist"**, which reads like a lifetime bug and is not one. Whatever owns the
  device must own the instance.
- egui delivers a texture as one allocation then partial updates. If the frame
  loop skips a frame's `textures_delta` — e.g. on an early return when the
  surface times out during startup, which happens in normal operation — the
  allocation is lost and the next partial update panics with **"Tried to update a
  texture that has not been allocated yet"**. The `Missing texture: Managed(0)`
  warnings are the same bug being polite first. Apply deltas before acquiring the
  surface.
- **WGSL `vec3<f32>` is 16-byte aligned.** A `vec3` padding field in a uniform
  block does not pack like the Rust `[f32; 3]` beside it — it pushes everything
  after it and changes the block size (112 vs 96 bytes here). Pad with scalars.

**macOS filesystems are case-insensitive**, so a `.app` bundle cannot hold both
`UnMapper` and `unmapper` in `Contents/MacOS` — the second copy silently replaces
the first. UnMapper's bundle shipped the CLI as its GUI until a launch check
caught it; the app started, printed usage to a log, and exited, which looks
exactly like a crash. Name bundled CLIs distinctly (`unmapper-cli`).

**The NDI machine-name prefix is not stable across sessions** — this Mac has
advertised as both `AZLAN-1386.LOCAL` and `MAC`. A probe using a remembered full
name reports `connected=false, 0 frames`, which looks exactly like a broken
sender. Always re-list sources and copy the full name.

Also: a live NDI sender for testing is usually already running on this Mac —
[weblinked](https://github.com/stoatworks-labs/weblinked/blob/main/docs/NOTES.md) (`weblinked`) publishes `WebLinkedBars` (colour bars with a frame
counter), which is ideal for verifying a receive path end to end.
