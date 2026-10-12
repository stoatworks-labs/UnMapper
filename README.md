# UnMapper

> **AI-assisted project.** This codebase was created with [Claude](https://claude.com/claude-code)
> (Anthropic), directed and reviewed by a human author. The pipeline is built and
> verified end to end on real hardware: a real Arena 7.27 slice map, a live NDI
> sender, a real GPU, and both canvases confirmed by pixel readback. **Nothing has
> ever run on an actual LED wall**, and the warp lattice is built on a hand-made
> fixture because there wasn't a properly warped Advanced Output to test with. See
> [Status](#status).

Recreate a physical LED rig on the screens you've actually got, and play the real
show onto it.

UnMapper reads a **Resolume Arena Advanced Output** (the same slice map that drives
the real wall), receives Resolume's outputs over **NDI**, and composites them onto
a virtual rebuild of the rig. That rebuild can go out to directly connected
displays, so a bank of monitors stands in for the LED, or be rendered as a 3D
previz of the set.

Two views of the same stage, and you get to use both:

- **Emulation:** the whole rig recreated flat, one canvas pixel per LED. Each
  connected display shows a cropped region, so a grid of monitors becomes the
  wall. It's pixel-exact: the crop is sampled **nearest**, so a region that isn't
  the monitor's own size looks blocky rather than being interpolated behind your
  back into something plausible.
- **Previz:** the same panels at their positions in 3D, through a camera.

Both are fed by the same slice map and the same live frames, so what you see in
one is what the other is showing.

## Status

Early days. The pipeline below is **built and verified end to end on real
hardware** (a real Arena 7.27 file, a live NDI sender, a real GPU):

| Piece | State |
|---|---|
| Resolume Advanced Output import | Built. Tested against 4 real files. |
| NDI discovery / receive / send | Built. Verified against a live NDI 6.3.2 sender at 50 fps. |
| Emulation canvas (wgpu) | Built. Verified by pixel readback. |
| 3D previz camera (wgpu) | Built. Verified by pixel readback. |
| Corner-pinned slice sampling | Built (projective). |
| Warp lattices | Built — a warped slice is drawn through its lattice, one quad per cell. See the caveat below. |
| Stage file (XML) | Built. Lossless round trip, hand-editable. |
| Desktop GUI | Built. Live NDI in the viewport, drag-to-place, save/load. |
| Output to connected displays | Built. One window per monitor, each a pixel-exact crop. |
| 2D backdrop mockup | Built. Viewport only — never reaches an output. |
| 3D set model (glTF/GLB) | Built. Node transforms baked, depth-tested with the panels. |
| NDI output | Built. Publishes a canvas region, or the previz camera, as an NDI source. |
| Previz to an output | Built. A window or an NDI source showing the camera view. |
| CLI | Built — `import`, `bind`, `check`, `render`, `sources`. |
| **Syphon / Spout publishing** | **Not built yet** — use NDI output instead. |
| Non-planar panels | Built — a panel surface can be flat, an arc, or a measured lattice. Previz only. |

Nothing has been run on a real LED wall or in a venue.

### Curved and folded panels

UnMapper imports one panel per slice, and a slice routinely covers a whole run of
tiles: a curved upstage wall, a wrapped column, a folded corner. Drawing that as
one flat rectangle is the very thing previz is meant to fix, so a panel carries a
**surface**: flat, an arc of a given sweep, or an explicit lattice of measured
points.

Surfaces only shape the **previz** view. The emulation canvas stays flat and
pixel-exact whatever the surface says, because one canvas pixel is one LED and an
output crops that canvas to a monitor standing in for a piece of the rig.

### About the warp lattices

An operator warps a slice in Resolume when the surface it feeds isn't a flat
rectangle: the content is pre-distorted so it lands straight on a curved wall.
The LED processor still reads a plain rectangle out of the raster, so showing
that rectangle shows you the pre-distorted image, which is what goes down the
wire and not what an audience ever sees. UnMapper reads *through* the lattice
instead, so the panel shows what the wall will show.

The limit, and it's a big one: **every Advanced Output to hand while this was
written had an untouched lattice on every slice.** Where the control points live
and what order they come in are pinned against real Arena 7.27 files, because an
untouched lattice is a regular grid and that's checkable. What a warped export
actually looks like isn't: the warped test fixture was made by hand and says so
in its header. A `Point Mode` other than `PM_LINEAR`, and a homography that does
more than restate the output rect, are both reported and not applied.

## Try it

The GUI is the main way in. Import an Advanced Output, pick an NDI source for
each Resolume output, drag the panels into the shape of your rig, and save.

```bash
cargo run -p unmapper-gui
```

It can also open a stage directly:

```bash
cargo run -p unmapper-gui -- rig.unmapper.xml
```

Everything it does is available headless too:

```bash
cargo run -p unmapper-app -- sources
```

```bash
cargo run -p unmapper-app -- import "AdvancedOutput.xml" -o rig.unmapper.xml
```

```bash
cargo run -p unmapper-app -- bind rig.unmapper.xml --source 0 --ndi "STUDIO (Arena - Screen 1)"
```

```bash
cargo run -p unmapper-app -- render rig.unmapper.xml -o wall.png
```

```bash
cargo run -p unmapper-app -- render rig.unmapper.xml --previz -o previz.png --size 1280x720
```

## The five coordinate spaces

Naming these is most of the battle, and mixing two of them up is the difference
between a correct wall and a plausible-looking wrong one.

| Space | Units | Holds |
|---|---|---|
| Composition | px | Resolume's composition raster. A slice's `input` quad. |
| Screen raster | px | One Resolume output. A slice's `output` quad. |
| Virtual raster | px | UnMapper's emulation canvas — the whole rig, flat. A panel's `layout`. |
| Stage | metres | The physical set, Y up. A panel's `placement`. |
| Display | px | A connected monitor. An output's `region` crops the canvas into one. |

**The one thing to get right:** a slice's pixels live in *two* places, and which
one to sample depends entirely on what the sender is sending. If Resolume sends
its whole composition, sample the slice's `input` quad. If it sends one feed per
screen (the usual show configuration), that feed already has the slicing applied,
so sample the `output` quad. `SourceSpace` records which.

## Geometry

There are two ways to describe the set, and if you've got both you don't have to
choose.

**A 2D mockup:** a render, plan or photo of the display surface, sitting behind
the panels on the emulation canvas so you can drag them onto the places they
occupy in it. It's an **editing aid and never content**: the viewport draws it,
and the canvas the outputs crop from doesn't. Fade it with the opacity slider so
the panels stay readable over a busy render.

**A 3D set model:** glTF or GLB, as exported by Blender, Cinema 4D, SketchUp or
anything else. Only geometry is read; materials, cameras and animation are
ignored, because this is context for judging where the walls sit, not a render.
Nested node transforms are baked at load, so a truss rotated inside a rig group
arrives where the file says it is. CAD is usually in millimetres, so there's a
one-click `mm→m` button for just that.

```xml
<Geometry>
  <Backdrop path="art/set-render.png" opacity="0.6">
    <Rect x="0" y="0" width="1920" height="1080"/>
  </Backdrop>
  <Model path="cad/set.glb" scale="0.001">
    <Translation x="0" y="0" z="0"/>
    <Rotation x="0" y="0" z="0" w="1"/>
  </Model>
</Geometry>
```

## Working with nothing plugged in

A source doesn't have to be NDI. There are two offline kinds, so a rig can be laid
out and checked with no Resolume, no network and often no venue:

- **Test pattern:** a grid with four differently-coloured corners and a centre
  cross, sized to whatever the slice map says the screen is. Because the corners
  differ, a slice that's flipped, rotated or sampling the wrong region is obvious
  at a glance rather than plausible-looking. It's a *geometry* aid, not a colour
  reference; `test-card` in this fleet does that properly.
- **Still:** any image, for laying a rig out against real artwork.

```xml
<Source id="src-9001" name="LED Processor 1" enabled="true">
  <TestPattern/>
  <ScreenRaster screen="9001"/>
  <Expected width="1920" height="1080"/>
</Source>
```

## Outputs

The canvas is rendered **once** per frame at full resolution, and every output
then blits the region it stands in for. Ten monitors cost one render and ten
blits, and none of them can disagree about which frame they're showing.

Add outputs in the GUI's Outputs panel, or by hand:

```xml
<Output id="out-left" name="Wall Left monitor" enabled="true">
  <Display index="0" fullscreen="true"/>
  <Emulation x="0" y="0" width="960" height="1080"/>
  <Size width="960" height="1080"/>
</Output>
```

New outputs are created **windowed**, not fullscreen, because a fullscreen window
that opens on the wrong monitor is a right pain to get rid of. Tick the box once
it looks right. Closing an output window disables that output rather than
quitting.

## The stage file

A stage saves as XML you can read, diff and hand-edit:

```xml
<UnMapperStage version="1" name="two-panel-wall">
  <VirtualRaster width="1920" height="1080"/>
  <Sources>
    <Source id="src-9001" name="LED Processor 1" enabled="true">
      <Ndi name="STUDIO (Arena - Screen 1)"/>
      <ScreenRaster screen="9001"/>
      <Expected width="1920" height="1080"/>
    </Source>
  </Sources>
  <Panels>
    <Panel id="panel-9001-9101" name="Wall Left" enabled="true">
      <Pixels width="960" height="1080"/>
      <Layout x="0" y="0" width="960" height="1080"/>
      <Placement>
        <Translation x="-1.248" y="1.404" z="0"/>
        <Rotation x="0" y="0" z="0" w="1"/>
        <Size width="2.496" height="2.808"/>
      </Placement>
    </Panel>
  </Panels>
  <Bindings>
    <Binding panel="panel-9001-9101" source="src-9001" slice="9101">
      <SourceQuad>
        <v x="0" y="0"/><v x="960" y="0"/><v x="960" y="1080"/><v x="0" y="1080"/>
      </SourceQuad>
    </Binding>
  </Bindings>
</UnMapperStage>
```

Quads are four `<v x= y=>` corners in Resolume's own order, so anyone who's read
an Advanced Output will recognise them. The round trip is exact: saving a stage
twice gives byte-identical files.

## Layout

```
crates/
  unmapper-core      domain model — spaces, panels, bindings, the show
  unmapper-resolume  Advanced Output reader
  unmapper-stagefile the stage XML format
  unmapper-ndi       NDI, loaded at run time rather than linked
  unmapper-render    wgpu — emulation canvas and previz camera
  unmapper-gui       the desktop application
  unmapper-app       the `unmapper` CLI
  diag               vendored fleet diagnostics
```

## NDI

The NDI runtime is **loaded at run time, never linked**. That's a licensing
requirement, not a build convenience: the NDI licence only allows redistribution
if the licence you ship under forbids reverse-engineering the SDK, and UnMapper is
MIT, which grants that very right. Loading at run time means no NDI code is
distributed and only the flat C ABI is named.

A machine with no runtime still builds and runs. NDI sources just aren't
available, and the error gives you the download URL. Install the
[NDI Tools or SDK](https://ndi.video/) to turn them on.

## Building a release

CI runs `cargo test` on every push to main and every pull request, on an Ubuntu
runner with `mesa-vulkan-drivers` installed so wgpu finds a software adapter and
the render tests can read back real pixels. It doesn't run `cargo fmt --check`
or `cargo clippy`, which are both red on main. There's no release job; the
release is still local:

```bash
scripts/release-local.sh
```

It runs the tests and clippy, builds a universal macOS binary, and wraps the GUI
in a double-clickable `UnMapper.app` with the CLI next to it. `--fast` builds this
machine's architecture only.

macOS only for now. Windows and Linux builds are possible with `cargo-xwin` and
`cargo-zigbuild`, but nothing here has ever been *run* on either, so shipping
binaries for them would suggest a confidence I haven't got.

The bundle is unsigned, so macOS quarantines it on first open: right-click →
Open, or `xattr -dr com.apple.quarantine dist-release/UnMapper.app`.

<!-- attributions:start -->
This project is built on other people's work — see [ATTRIBUTIONS.md](ATTRIBUTIONS.md).
<!-- attributions:end -->

## Licence

MIT. See [LICENSE](LICENSE).

NDI® is a registered trademark of Vizrt NDI AB. This project is not affiliated
with Vizrt, Resolume, or any LED manufacturer.
