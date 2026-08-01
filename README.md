# UnMapper

Recreate a physical LED rig on the screens you actually have, and play the real
show onto it.

UnMapper reads a **Resolume Arena Advanced Output** — the same slice map driving
the real wall — receives Resolume's outputs over **NDI**, and composites them
onto a virtual reconstruction of the rig. That reconstruction can be driven out
to directly connected displays, so a bank of monitors stands in for the LED, or
rendered as a 3D previz of the set.

Two views of one stage, and they are not alternatives:

- **Emulation** — the whole rig recreated flat, one canvas pixel per LED. Each
  connected display shows a cropped region, so a grid of monitors becomes the
  wall. Pixel-exact: the crop is sampled **nearest**, so a region that is not the
  monitor's own size looks blocky rather than being quietly interpolated into
  something plausible.
- **Previz** — the same panels at their positions in 3D, through a camera.

Both are fed by the same slice map and the same live frames, so what you see in
one is what the other is showing.

> Built with AI assistance (Claude).

## Status

Early. The pipeline below is **built and verified end to end on real hardware** —
a real Arena 7.27 file, a live NDI sender, a real GPU:

| Piece | State |
|---|---|
| Resolume Advanced Output import | Built. Tested against 4 real files. |
| NDI discovery / receive / send | Built. Verified against a live NDI 6.3.2 sender at 50 fps. |
| Emulation canvas (wgpu) | Built. Verified by pixel readback. |
| 3D previz camera (wgpu) | Built. Verified by pixel readback. |
| Corner-pinned slice sampling | Built (projective). |
| Stage file (XML) | Built. Lossless round trip, hand-editable. |
| Desktop GUI | Built. Live NDI in the viewport, drag-to-place, save/load. |
| Output to connected displays | Built. One window per monitor, each a pixel-exact crop. |
| 2D backdrop mockup | Built. Viewport only — never reaches an output. |
| 3D set model (glTF/GLB) | Built. Node transforms baked, depth-tested with the panels. |
| NDI output | Built. Publishes a canvas region as an NDI source. |
| CLI | Built — `import`, `bind`, `check`, `render`, `sources`. |
| **Previz to an output window** | **Not built yet** — previz is viewport and PNG only. |
| **Syphon / Spout publishing** | **Not built yet** — use NDI output instead. |

Nothing has been run on a real LED wall or in a venue.

## Try it

The GUI is the intended way in. Import an Advanced Output, pick an NDI source
for each Resolume output, drag the panels into the shape of your rig, and save.

```bash
cargo run -p unmapper-gui
```

It also opens a stage directly:

```bash
cargo run -p unmapper-gui -- rig.unmapper.xml
```

Everything it does is available headlessly too:

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
screen — the usual show configuration — that feed already has the slicing
applied, so sample the `output` quad. `SourceSpace` records which.

## Geometry

Two ways to describe the set, and they are not alternatives — an operator with
both should not have to choose.

**A 2D mockup** — a render, plan or photo of the display surface, sitting behind
the panels on the emulation canvas so they can be dragged onto the places they
occupy in it. It is an **editing aid and never content**: the viewport draws it,
and the canvas that outputs crop from does not. Fade it with the opacity slider
so panels stay readable over a busy render.

**A 3D set model** — glTF or GLB, as exported by Blender, Cinema 4D, SketchUp or
anything else. Only geometry is read; materials, cameras and animation are
ignored, because this is context for judging where the walls sit, not a render.
Nested node transforms are baked at load, so a truss rotated inside a rig group
arrives where the file says it is. CAD is usually in millimetres — there is a
one-click `mm→m` button for exactly that.

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

## Outputs

The canvas is rendered **once** per frame at full resolution; every output then
blits the region it stands in for. Ten monitors cost one render and ten blits,
and none of them can disagree about which frame they are showing.

Add outputs in the GUI's Outputs panel, or by hand:

```xml
<Output id="out-left" name="Wall Left monitor" enabled="true">
  <Display index="0" fullscreen="true"/>
  <Emulation x="0" y="0" width="960" height="1080"/>
  <Size width="960" height="1080"/>
</Output>
```

New outputs are created **windowed**, not fullscreen — a fullscreen window that
opens on the wrong monitor is unpleasant to get rid of. Tick the box once it
looks right. Closing an output window disables that output rather than quitting.

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

Quads are four `<v x= y=>` corners in Resolume's own order, so anyone who has
read an Advanced Output recognises them. The round trip is exact — saving a
stage twice gives byte-identical files.

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

The NDI runtime is **loaded at run time, never linked**. That is a licensing
requirement, not a build convenience: the NDI licence permits redistribution only
if the licence you ship under forbids reverse-engineering the SDK, and UnMapper
is MIT, which grants exactly that right. Loading at run time means no NDI code is
distributed and only the flat C ABI is named.

A machine with no runtime still builds and runs — NDI sources are simply
unavailable, with the download URL in the error. Install the
[NDI Tools or SDK](https://ndi.video/) to enable them.

## Licence

MIT. See [LICENSE](LICENSE).

NDI® is a registered trademark of Vizrt NDI AB. This project is not affiliated
with Vizrt, Resolume, or any LED manufacturer.
