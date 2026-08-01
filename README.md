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
  wall. Pixel-exact.
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
| CLI | Built — `import`, `bind`, `check`, `render`, `sources`. |
| **GUI** | **Not built yet.** |
| **Output to connected displays** | **Not built yet** — renders to PNG today. |
| **Syphon / Spout publishing** | **Not built yet.** |
| **3D CAD model + 2D backdrop loading** | **Modelled, not loaded yet.** |

Nothing has been run on a real LED wall or in a venue.

## Try it

```bash
cargo run -p unmapper-app -- sources
```

```bash
cargo run -p unmapper-app -- import "AdvancedOutput.xml" -o rig.unmapper.json
```

```bash
cargo run -p unmapper-app -- bind rig.unmapper.json --source 0 --ndi "STUDIO (Arena - Screen 1)"
```

```bash
cargo run -p unmapper-app -- render rig.unmapper.json -o wall.png
```

```bash
cargo run -p unmapper-app -- render rig.unmapper.json --previz -o previz.png --size 1280x720
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

## Layout

```
crates/
  unmapper-core      domain model — spaces, panels, bindings, the show file
  unmapper-resolume  Advanced Output reader
  unmapper-ndi       NDI, loaded at run time rather than linked
  unmapper-render    wgpu — emulation canvas and previz camera
  unmapper-app       the `unmapper` binary
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
