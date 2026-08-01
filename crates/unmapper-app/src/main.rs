//! UnMapper's command line.
//!
//! The GUI is the intended way to use UnMapper, but every step it performs is
//! available here too — and this is how the whole chain gets verified end to end
//! on a real machine, with a real Resolume file and a real NDI sender, without
//! anyone having to look at a window and decide whether it looks right.
//!
//!     unmapper sources
//!     unmapper import "Advanced Output.xml" -o rig.unmapper.xml
//!     unmapper bind rig.unmapper.xml --source 0 --ndi "STUDIO (Arena - Screen 1)"
//!     unmapper render rig.unmapper.xml -o frame.png

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use unmapper_core::{Camera, Severity, Show, Size, SourceKind, DEFAULT_PITCH_MM};
use unmapper_render::{
    build_canvas_scene, build_previz_scene, Gpu, Model, RenderTarget, Renderer, SourceTextures,
    DEPTH_FORMAT,
};

#[derive(Parser)]
#[command(
    name = "unmapper",
    version,
    about = "Recreate an LED rig and play NDI onto it"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List NDI senders visible on this network.
    Sources {
        /// How long to look. Discovery is not instant.
        #[arg(long, default_value_t = 3)]
        seconds: u64,
    },
    /// Read a Resolume Advanced Output and build a show from it.
    Import {
        /// An AdvancedOutput.xml or an Advanced Output preset.
        file: PathBuf,
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// LED pixel pitch in millimetres, used to give panels a physical size.
        #[arg(long, default_value_t = DEFAULT_PITCH_MM)]
        pitch: f32,
    },
    /// Point one of a show's sources at an NDI sender.
    Bind {
        show: PathBuf,
        /// Source index, as printed by `import`.
        #[arg(long)]
        source: usize,
        /// The sender's full NDI name.
        #[arg(long)]
        ndi: String,
    },
    /// Describe a show and report anything wrong with it.
    Check { show: PathBuf },
    /// Render a show to a PNG — the emulation canvas, or the previz view.
    Render {
        show: PathBuf,
        #[arg(short, long, default_value = "unmapper.png")]
        out: PathBuf,
        /// Render the 3D previz view instead of the flat emulation canvas.
        #[arg(long)]
        previz: bool,
        /// Previz output size.
        #[arg(long, default_value = "1280x720")]
        size: String,
        /// Wait up to this long for a frame from each bound NDI source.
        #[arg(long, default_value_t = 5)]
        wait: u64,
    },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Sources { seconds } => sources(seconds),
        Command::Import { file, out, pitch } => import(&file, out.as_deref(), pitch),
        Command::Bind { show, source, ndi } => bind(&show, source, &ndi),
        Command::Check { show } => check(&show),
        Command::Render {
            show,
            out,
            previz,
            size,
            wait,
        } => render(&show, &out, previz, &size, wait),
    }
}

fn load_show(path: &Path) -> Result<Show> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    unmapper_stagefile::from_xml(&text).with_context(|| format!("reading {}", path.display()))
}

fn save_show(show: &Show, path: &Path) -> Result<()> {
    let xml = unmapper_stagefile::to_xml(show)?;
    std::fs::write(path, xml).with_context(|| format!("writing {}", path.display()))
}

fn sources(seconds: u64) -> Result<()> {
    let ndi = unmapper_ndi::Ndi::load()?;
    println!(
        "NDI runtime {} at {}",
        ndi.version(),
        ndi.library_path().display()
    );
    let found = ndi.discover(Duration::from_secs(seconds))?;
    if found.is_empty() {
        println!("no senders found — discovery is not instant, so try a longer --seconds");
        return Ok(());
    }
    for s in found {
        println!("  {}", s.name);
    }
    Ok(())
}

fn import(file: &Path, out: Option<&Path>, pitch: f32) -> Result<()> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    if !unmapper_resolume::is_resolume_xml(&text) {
        bail!(
            "{} does not look like a Resolume advanced output (no <ScreenSetup> or <XmlState>)",
            file.display()
        );
    }

    let name = file.file_name().unwrap_or_default().to_string_lossy();
    let map = unmapper_resolume::parse(&text, &name)?;

    println!("{}", map.project_name);
    if let Some(c) = map.composition {
        println!("  composition {}x{}", c.width, c.height);
    }
    for w in &map.warnings {
        println!("  ! {w}");
    }
    for screen in &map.screens {
        println!(
            "  screen {:?}: {}x{} ({:?}), {} slice(s)",
            screen.name,
            screen.raster.width,
            screen.raster.height,
            screen.raster_source,
            screen.slices.len()
        );
        for note in &screen.notes {
            println!("    ! {note}");
        }
    }

    let show = Show::from_slice_map(map, pitch);
    println!(
        "\n{} panel(s) on a {}x{} canvas, {} source(s):",
        show.panels.len(),
        show.virtual_raster.width,
        show.virtual_raster.height,
        show.sources.len()
    );
    for (i, s) in show.sources.iter().enumerate() {
        println!("  [{i}] {:?} — {}", s.name, describe_source(&s.kind));
    }

    let path = out.map(Path::to_path_buf).unwrap_or_else(|| {
        PathBuf::from(format!(
            "{}.{}",
            show.name,
            unmapper_stagefile::STAGE_EXTENSION
        ))
    });
    save_show(&show, &path)?;
    println!("\nwrote {}", path.display());
    println!(
        "Bind a source with:  unmapper bind {} --source 0 --ndi \"…\"",
        path.display()
    );
    Ok(())
}

fn describe_source(kind: &SourceKind) -> String {
    match kind {
        SourceKind::Ndi { name } => format!("NDI {name:?}"),
        SourceKind::TestPattern => "unbound (test pattern)".into(),
        SourceKind::Still { path } => format!("still {}", path.display()),
    }
}

fn bind(path: &Path, index: usize, ndi_name: &str) -> Result<()> {
    let mut show = load_show(path)?;
    let count = show.sources.len();
    let source = show
        .sources
        .get_mut(index)
        .with_context(|| format!("this show has {count} source(s), so there is no [{index}]"))?;
    source.kind = SourceKind::Ndi {
        name: ndi_name.to_owned(),
    };
    println!("source [{index}] {:?} → NDI {ndi_name:?}", source.name);
    save_show(&show, path)
}

fn check(path: &Path) -> Result<()> {
    let show = load_show(path)?;
    println!(
        "{}: {} panel(s), {} source(s), {}x{} canvas",
        show.name,
        show.panels.len(),
        show.sources.len(),
        show.virtual_raster.width,
        show.virtual_raster.height
    );
    for (i, s) in show.sources.iter().enumerate() {
        println!("  [{i}] {:?} — {}", s.name, describe_source(&s.kind));
    }

    let problems = show.validate();
    if problems.is_empty() {
        println!("\nno problems found");
        return Ok(());
    }
    for p in &problems {
        let tag = match p.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        println!("  {tag}: {}", p.message);
    }
    if problems.iter().any(|p| p.severity == Severity::Error) {
        bail!("this show has errors and will not render correctly");
    }
    Ok(())
}

fn parse_size(s: &str) -> Result<Size> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .with_context(|| format!("expected WIDTHxHEIGHT, got {s:?}"))?;
    Ok(Size::new(w.trim().parse()?, h.trim().parse()?))
}

fn render(path: &Path, out: &Path, previz: bool, size: &str, wait: u64) -> Result<()> {
    let show = load_show(path)?;

    let gpu = Gpu::new_blocking()?;
    println!("GPU: {} ({:?})", gpu.adapter_name, gpu.backend);

    let mut textures = SourceTextures::new(&gpu);
    let mut renderer = Renderer::new(&gpu, &textures.layout);

    // Connect every NDI-bound source and wait for one frame from each. A source
    // that never delivers is reported rather than silently rendering dim — a
    // black wall in a venue is exactly the thing this tool exists to catch early.
    let ndi_sources: Vec<(String, String)> = show
        .sources
        .iter()
        .filter(|s| s.enabled)
        .filter_map(|s| match &s.kind {
            SourceKind::Ndi { name } => Some((s.id.clone(), name.clone())),
            _ => None,
        })
        .collect();

    if !ndi_sources.is_empty() {
        let ndi = unmapper_ndi::Ndi::load()?;
        let mut receivers = Vec::new();
        for (id, name) in &ndi_sources {
            println!("connecting {id} → {name:?}");
            receivers.push((id.clone(), ndi.receive(name, "UnMapper")));
        }

        let deadline = Instant::now() + Duration::from_secs(wait);
        let mut pending: Vec<usize> = (0..receivers.len()).collect();
        while !pending.is_empty() && Instant::now() < deadline {
            pending.retain(|&i| {
                let (id, recv) = &receivers[i];
                match recv.take_frame() {
                    Some(frame) => {
                        println!(
                            "  {id}: {}x{} {}",
                            frame.width,
                            frame.height,
                            frame.format.label()
                        );
                        // A feed that is not the size the slice map says the
                        // screen is will sample the wrong regions of it — slices
                        // near the far edge clamp, and the wall comes out wrong
                        // in a way that is easy to misread as a mapping mistake.
                        // Catching it here is most of the value of the tool.
                        if let Some(expected) = show.source(id).and_then(|s| s.expected) {
                            if expected.width != frame.width || expected.height != frame.height {
                                println!(
                                    "  ! {id}: the slice map expects {}x{} but this sender is \
                                     sending {}x{}. Slices will sample the wrong regions — set \
                                     the Resolume output to match, or correct the screen raster.",
                                    expected.width, expected.height, frame.width, frame.height
                                );
                            }
                        }
                        textures.upload(
                            &gpu,
                            id,
                            unmapper_render::FrameUpload {
                                width: frame.width,
                                height: frame.height,
                                stride: frame.stride,
                                bgra: frame.format == unmapper_ndi::PixelFormat::Bgra,
                                data: &frame.data,
                                sequence: frame.sequence,
                            },
                        );
                        recv.recycle(frame.data);
                        false
                    }
                    None => true,
                }
            });
            std::thread::sleep(Duration::from_millis(10));
        }
        for i in pending {
            let (id, recv) = &receivers[i];
            let status = recv.status();
            println!(
                "  ! {id}: no frame within {wait}s (connected={}{})",
                status.connected,
                status
                    .last_error
                    .map(|e| format!(", {e}"))
                    .unwrap_or_default()
            );
        }
    }

    let (target_size, data) = if previz {
        let size = parse_size(size)?;
        let camera = default_camera(&show);
        let scene = build_previz_scene(&show, &textures);

        // The set model, if the stage names one. A file that fails to load is
        // reported and skipped rather than aborting the render — the panels are
        // the point, the scenery is context.
        let model = match &show.geometry.model {
            Some(m) if !m.path.as_os_str().is_empty() => {
                match unmapper_render::load_gltf(&m.path) {
                    Ok(mesh) => {
                        println!(
                            "model: {} triangle(s){}",
                            mesh.triangle_count(),
                            if mesh.skipped > 0 {
                                format!(", {} primitive(s) skipped", mesh.skipped)
                            } else {
                                String::new()
                            }
                        );
                        Some((Model::new(&gpu, &mesh, DEPTH_FORMAT), m.clone()))
                    }
                    Err(e) => {
                        println!("  ! could not load the model: {e:#}");
                        None
                    }
                }
            }
            _ => None,
        };

        let target = RenderTarget::new(&gpu, size, "previz");
        renderer.render_previz(
            &gpu,
            &target.view,
            size,
            unmapper_render::PrevizView {
                camera: &camera,
                model: model.as_ref().map(|(m, p)| (m, p)),
            },
            &scene,
            &textures,
        );
        (size, target.read_rgba(&gpu))
    } else {
        let size = show.virtual_raster;
        let scene = build_canvas_scene(&show, &textures);
        let target = RenderTarget::new(&gpu, size, "canvas");
        renderer.render_canvas(&gpu, &target.view, size, &scene, &textures);
        (size, target.read_rgba(&gpu))
    };

    let image = image::RgbaImage::from_raw(target_size.width, target_size.height, data)
        .context("the readback was not the size the render target reported")?;
    image
        .save(out)
        .with_context(|| format!("writing {}", out.display()))?;
    println!(
        "wrote {} ({}x{})",
        out.display(),
        target_size.width,
        target_size.height
    );
    Ok(())
}

/// A camera framing the whole rig, for a previz render with no camera saved yet.
fn default_camera(show: &Show) -> Camera {
    let mut camera = Camera::default();
    let enabled: Vec<_> = show.panels.iter().filter(|p| p.enabled).collect();
    if enabled.is_empty() {
        return camera;
    }

    let mut min = glam::Vec3::splat(f32::INFINITY);
    let mut max = glam::Vec3::splat(f32::NEG_INFINITY);
    for panel in &enabled {
        for c in panel.placement.corners() {
            min = min.min(c);
            max = max.max(c);
        }
    }
    let centre = (min + max) / 2.0;
    let extent = (max - min).max_element().max(1.0);

    camera.target = centre;
    // Far enough back that the whole rig fits the default lens, with a margin.
    camera.position = centre + glam::Vec3::new(0.0, 0.0, extent * 1.6);
    camera
}
