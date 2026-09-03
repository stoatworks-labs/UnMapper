//! UnMapper's desktop application.
//!
//! # How the two renderers share one window
//!
//! egui draws the chrome; UnMapper's own wgpu renderer draws the stage. Rather
//! than egui painting over a live surface, the stage is rendered into an
//! **offscreen target** which is then registered with `egui-wgpu` as a native
//! texture and drawn as an ordinary image in the central panel.
//!
//! That indirection buys three things: the viewport can be any size without
//! touching the swapchain, the stage image is the same one that will later feed a
//! display output or a texture-share publisher, and egui's own painting never has
//! to interleave with UnMapper's render passes.

mod about_data;
// Vendored from stoatworks-backend/about/rust, which is the master: the copy
// here must not be edited, so its one clippy nit is silenced at the import
// instead. Fix it there and re-run sync-about.py, and this can go.
#[allow(clippy::redundant_closure)]
mod about_window;
mod outputs;
mod state;
mod ui;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use unmapper_core::Size;
use unmapper_render::{
    build_canvas_scene, build_previz_scene, build_viewport_scene, FrameUpload, Gpu, Model,
    RenderTarget, Renderer, SourceTextures, BACKDROP_ID, DEPTH_FORMAT,
};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use state::{App, ViewMode};

fn main() -> Result<()> {
    let _guard = diag::init(
        diag::Options::new("unmapper-gui", "UNMAPPER", env!("CARGO_PKG_VERSION"))
            .with_default_filter("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"),
    )?;

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut host = Host::default();

    // `unmapper-gui rig.unmapper.xml` — opening from the shell, from a file
    // manager's "open with", and from a test script all go through here.
    if let Some(arg) = std::env::args().nth(1) {
        let path = std::path::PathBuf::from(arg);
        match std::fs::read_to_string(&path)
            .context("reading the file")
            .and_then(|t| unmapper_stagefile::from_xml(&t).map_err(Into::into))
        {
            Ok(show) => host.app.replace_show(show, Some(path)),
            Err(e) => {
                // Worth starting anyway with an empty stage, but say why.
                eprintln!("could not open that stage: {e:#}");
                host.app.error(format!("could not open that stage: {e:#}"));
            }
        }
    }

    event_loop.run_app(&mut host)?;
    Ok(())
}

/// Everything that only exists once a window does.
struct Live {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    gpu: Gpu,

    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,

    renderer: Renderer,
    textures: SourceTextures,
    /// The emulation canvas at full resolution. Rendered once per frame; the
    /// viewport and every output window are crops of it, so N monitors cost one
    /// render and N blits.
    canvas: RenderTarget,
    /// A previz render for outputs that show the camera view rather than a crop
    /// of the canvas. Separate from the viewport's target, which is sized to the
    /// window rather than to what an output asked for.
    previz_target: Option<RenderTarget>,
    /// The backdrop path currently uploaded, so the image is read from disk once
    /// rather than every frame.
    backdrop_loaded: Option<std::path::PathBuf>,
    /// The set model, and the path it came from. Same once-per-path rule.
    model: Option<Model>,
    model_loaded: Option<std::path::PathBuf>,
    target: RenderTarget,
    target_id: egui::TextureId,

    last_title: String,
}

#[derive(Default)]
struct Host {
    live: Option<Live>,
    app: App,
    /// The windows standing in for the wall.
    output_windows: outputs::OutputWindows,
    /// Discovery blocks, so it runs on its own thread and reports back here.
    discovery: Option<std::sync::mpsc::Receiver<Vec<unmapper_ndi::SourceName>>>,
    /// Set by the File menu; acted on after the frame, since the event loop can
    /// only be told to exit from the handler.
    quit: bool,
    /// Same for display enumeration, which also needs the event loop.
    rescan_displays: bool,
}

impl ApplicationHandler for Host {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.live.is_some() {
            return;
        }
        self.app.monitors = outputs::enumerate_monitors(event_loop);
        match pollster::block_on(Live::new(event_loop, &self.app)) {
            Ok(live) => self.live = Some(live),
            Err(e) => {
                // Nothing useful can happen without a GPU, and a silent exit
                // would look like a crash.
                eprintln!("UnMapper could not start: {e:#}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(live) = &mut self.live else { return };

        // Output windows are not the main window: they get no egui input, and
        // closing one stops that output rather than quitting the application.
        if self.output_windows.owns(id) {
            match event {
                WindowEvent::CloseRequested => {
                    if let Some(output_id) = self.output_windows.close(id) {
                        if let Some(o) =
                            self.app.show.outputs.iter_mut().find(|o| o.id == output_id)
                        {
                            o.enabled = false;
                            self.app.dirty = true;
                        }
                        self.app.toast("Output closed");
                    }
                }
                WindowEvent::Resized(size) => {
                    self.output_windows
                        .resize(&live.gpu, id, size.width, size.height);
                }
                _ => {}
            }
            return;
        }

        // egui gets first refusal on every event; anything it consumes must not
        // also drive the viewport underneath.
        let response = live.egui_state.on_window_event(&live.window, &event);
        if response.repaint {
            live.window.request_redraw();
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                live.resize(size.width, size.height);
                live.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                self.poll_discovery();
                if let Err(e) = self.frame() {
                    tracing::error!("frame failed: {e:#}");
                }
                if self.quit {
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Video is arriving continuously, so redraw continuously rather than
        // waiting for input.
        if let Some(live) = &self.live {
            live.window.request_redraw();
        }

        if self.rescan_displays {
            self.rescan_displays = false;
            self.app.monitors = outputs::enumerate_monitors(event_loop);
            let n = self.app.monitors.len();
            self.app.toast(format!("Found {n} display(s)"));
        }

        // Windows can only be created from the event loop, so output windows are
        // reconciled here rather than inside the frame.
        if let Some(live) = &self.live {
            let monitors = self.app.monitors.clone();
            let messages =
                self.output_windows
                    .sync(event_loop, &live.gpu, &self.app.show, &monitors);
            for m in messages {
                self.app.error(m);
            }
        }
        self.output_windows.request_redraw();
    }
}

impl Host {
    fn poll_discovery(&mut self) {
        let Some(rx) = &self.discovery else { return };
        match rx.try_recv() {
            Ok(found) => {
                self.app.discovered = found;
                self.app.discovering = false;
                self.discovery = None;
                let n = self.app.discovered.len();
                self.app.toast(format!("Found {n} NDI source(s)"));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.app.discovering = false;
                self.discovery = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    fn start_discovery(&mut self) {
        let Some(ndi) = self.app.ndi.clone() else {
            return;
        };
        if self.app.discovering {
            return;
        }
        self.app.discovering = true;
        let (tx, rx) = std::sync::mpsc::channel();
        self.discovery = Some(rx);
        std::thread::Builder::new()
            .name("ndi-discovery".into())
            .spawn(move || {
                let found = ndi.discover(Duration::from_secs(3)).unwrap_or_default();
                let _ = tx.send(found);
            })
            .expect("spawning discovery");
    }

    fn frame(&mut self) -> Result<()> {
        let Some(live) = &mut self.live else {
            return Ok(());
        };

        self.app.prune_toasts();
        self.app.sync_receivers();

        // Pull whatever has arrived since the last frame and upload it. Newest
        // wins; the receiver has already dropped anything older.
        let ids: Vec<String> = self.app.receivers().map(|(id, _)| id.clone()).collect();
        for id in ids {
            let Some(recv) = self.app.receiver(&id) else {
                continue;
            };
            if let Some(frame) = recv.take_frame() {
                live.textures.upload(
                    &live.gpu,
                    &id,
                    FrameUpload {
                        width: frame.width,
                        height: frame.height,
                        stride: frame.stride,
                        bgra: frame.format == unmapper_ndi::PixelFormat::Bgra,
                        data: &frame.data,
                        sequence: frame.sequence,
                    },
                );
                recv.recycle(frame.data);
            }
        }

        let title = self.app.title();
        if title != live.last_title {
            live.window.set_title(&title);
            live.last_title = title;
        }

        // --- egui pass ------------------------------------------------------
        let raw_input = live.egui_state.take_egui_input(&live.window);
        let mut actions = ui::Actions::default();

        // Panel order is load-bearing in egui: the first added is outermost and
        // the CentralPanel must be last, or the viewport eats the whole window.
        let size = (live.target.size.width, live.target.size.height);
        let target_id = live.target_id;
        let open_outputs = self.output_windows.open_count() + self.output_windows.ndi_count();
        let app = &mut self.app;
        let full_output = live.egui_ctx.clone().run_ui(raw_input, |ui| {
            ui::menu_bar(ui, app, &mut actions);
            ui::status_bar(ui, app, open_outputs);
            ui::sources_panel(ui, app, &mut actions);
            ui::inspector_panel(ui, app);
            ui::viewport(ui, app, target_id, size);
            // Last, and on the context rather than inside a panel, so it floats
            // over the viewport instead of being laid out beside it.
            crate::about_window::show(ui.ctx(), &mut app.show_about);
        });

        live.egui_state
            .handle_platform_output(&live.window, full_output.platform_output);

        // --- stage pass -----------------------------------------------------
        live.ensure_target(live.viewport_size_hint(self.app.viewport_px));

        if let Err(e) = live.sync_backdrop(&self.app.show) {
            self.app.error(format!("{e:#}"));
            // Drop the path rather than retrying the same broken file every
            // frame at 60Hz.
            self.app.show.geometry.backdrop = None;
        }
        let (loaded, errors) =
            unmapper_render::sync_offline_sources(&live.gpu, &mut live.textures, &self.app.show);
        for u in loaded {
            tracing::info!(source = %u.source_id, what = u.what, w = u.size.width, h = u.size.height, "offline source ready");
        }
        for e in errors {
            self.app.error(e);
        }

        match live.sync_model(&self.app.show) {
            Ok(Some(summary)) => self.app.toast(summary),
            Ok(None) => {}
            Err(e) => {
                self.app.error(format!("{e:#}"));
                self.app.show.geometry.model = None;
            }
        }

        // Outputs crop the canvas; the viewport renders its own view. These are
        // deliberately NOT the same image: the viewport carries editing aids —
        // the backdrop mockup today — that must never reach a monitor standing
        // in for the wall. Every output still shares one canvas render, so no
        // two of them can show different frames.
        let ndi_messages =
            self.output_windows
                .sync_ndi(&live.gpu, &self.app.show, self.app.ndi.as_ref());
        for m in ndi_messages {
            self.app.error(m);
        }

        if self.output_windows.needs_canvas() {
            live.ensure_canvas(self.app.show.virtual_raster);
            let canvas_scene = build_canvas_scene(&self.app.show, &live.textures);
            live.renderer.render_canvas(
                &live.gpu,
                &live.canvas.view,
                live.canvas.size,
                &canvas_scene,
                &live.textures,
            );
        }

        match self.app.mode {
            ViewMode::Canvas => {
                let scene = build_viewport_scene(&self.app.show, &live.textures);
                live.renderer.render_canvas_view(
                    &live.gpu,
                    &live.target,
                    unmapper_render::CanvasView::new(self.app.pan, self.app.zoom),
                    &scene,
                    &live.textures,
                );
            }
            ViewMode::Previz => {
                let camera = self.app.previz_camera();
                let scene = build_previz_scene(&self.app.show, &live.textures);
                let model = live
                    .model
                    .as_ref()
                    .zip(self.app.show.geometry.model.as_ref());
                live.renderer.render_previz(
                    &live.gpu,
                    &live.target.view,
                    live.target.size,
                    unmapper_render::PrevizView {
                        camera: &camera,
                        model,
                    },
                    &scene,
                    &live.textures,
                );
            }
        }

        // Outputs showing the previz camera need their own render — a camera view
        // is not a crop of anything.
        if let Some(size) = self.output_windows.previz_size() {
            if !matches!(&live.previz_target, Some(t) if t.size == size) {
                live.previz_target = Some(RenderTarget::new(&live.gpu, size, "previz output"));
            }
            let camera = self.app.previz_camera();
            let scene = build_previz_scene(&self.app.show, &live.textures);
            let model = live
                .model
                .as_ref()
                .zip(self.app.show.geometry.model.as_ref());
            let target = live.previz_target.as_ref().expect("just ensured");
            live.renderer.render_previz(
                &live.gpu,
                &target.view,
                size,
                unmapper_render::PrevizView {
                    camera: &camera,
                    model,
                },
                &scene,
                &live.textures,
            );
        } else {
            live.previz_target = None;
        }

        self.output_windows.render(
            &live.gpu,
            &live.canvas,
            live.previz_target.as_ref(),
            &self.app.show,
        );

        // --- present --------------------------------------------------------
        live.present(
            full_output.shapes,
            full_output.pixels_per_point,
            full_output.textures_delta,
        )?;

        // --- deferred actions ----------------------------------------------
        // Run after presenting: a file dialog blocks the thread, and doing it
        // mid-frame would leave the window unpainted behind the dialog.
        self.run_actions(actions);
        Ok(())
    }

    fn run_actions(&mut self, actions: ui::Actions) {
        if actions.discover {
            self.start_discovery();
        }
        if actions.pick_backdrop {
            self.pick_backdrop();
        }
        if actions.pick_model {
            self.pick_model();
        }
        if actions.rescan_displays {
            // Monitors can only be enumerated from the event loop, so this is
            // picked up in about_to_wait rather than acted on here.
            self.rescan_displays = true;
        }
        if actions.import_resolume {
            self.import_resolume();
        }
        if actions.open_stage {
            self.open_stage();
        }
        if actions.save {
            if let Some(path) = self.app.path.clone() {
                self.save_to(path);
            }
        }
        if actions.save_as {
            self.save_as();
        }
        if actions.quit {
            self.quit = true;
        }
    }

    fn import_resolume(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Resolume Advanced Output", &["xml"])
            .set_title("Import a Resolume Advanced Output")
            .pick_file()
        else {
            return;
        };
        match std::fs::read_to_string(&path)
            .context("reading the file")
            .and_then(|text| {
                if !unmapper_resolume::is_resolume_xml(&text) {
                    anyhow::bail!(
                        "that is not a Resolume advanced output (no <ScreenSetup> or <XmlState>)"
                    );
                }
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                unmapper_resolume::parse(&text, &name).map_err(Into::into)
            }) {
            Ok(map) => ui::apply_import(&mut self.app, map, path, unmapper_core::DEFAULT_PITCH_MM),
            Err(e) => self.app.error(format!("{e:#}")),
        }
    }

    fn pick_backdrop(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Image",
                &["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp"],
            )
            .set_title("Choose a backdrop image")
            .pick_file()
        else {
            return;
        };

        // Default the rect to the whole canvas: a mockup of the set is almost
        // always a picture of the whole thing, and an operator who wants it
        // somewhere else can drag the numbers.
        let raster = self.app.show.virtual_raster;
        self.app.show.geometry.backdrop = Some(unmapper_core::Backdrop {
            path,
            rect: unmapper_core::Rect::new(0.0, 0.0, raster.width as f32, raster.height as f32),
            opacity: 0.6,
        });
        self.app.dirty = true;
    }

    fn pick_model(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("glTF model", &["gltf", "glb"])
            .set_title("Choose a set model")
            .pick_file()
        else {
            return;
        };
        // Keep whatever scale and pose were already set, so replacing a model
        // with a re-export does not throw away the alignment work.
        let existing = self.app.show.geometry.model.clone().unwrap_or_default();
        self.app.show.geometry.model = Some(unmapper_core::Model3d { path, ..existing });
        self.app.dirty = true;
    }

    fn open_stage(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("UnMapper stage", &["xml"])
            .set_title("Open a stage")
            .pick_file()
        else {
            return;
        };
        match std::fs::read_to_string(&path)
            .context("reading the file")
            .and_then(|t| unmapper_stagefile::from_xml(&t).map_err(Into::into))
        {
            Ok(show) => {
                let n = show.panels.len();
                self.app.replace_show(show, Some(path));
                self.app.toast(format!("Opened {n} panel(s)"));
            }
            Err(e) => self.app.error(format!("{e:#}")),
        }
    }

    fn save_as(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("UnMapper stage", &["xml"])
            .set_file_name(format!("{}.unmapper.xml", self.app.show.name))
            .set_title("Save the stage")
            .save_file()
        else {
            return;
        };
        self.save_to(path);
    }

    fn save_to(&mut self, path: std::path::PathBuf) {
        match unmapper_stagefile::to_xml(&self.app.show)
            .map_err(anyhow::Error::from)
            .and_then(|xml| std::fs::write(&path, xml).map_err(Into::into))
        {
            Ok(()) => {
                self.app.dirty = false;
                self.app.path = Some(path.clone());
                self.app.toast(format!(
                    "Saved {}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
            Err(e) => self.app.error(format!("could not save: {e:#}")),
        }
    }
}

impl Live {
    async fn new(event_loop: &ActiveEventLoop, app: &App) -> Result<Self> {
        let attrs = Window::default_attributes()
            .with_title(app.title())
            .with_inner_size(winit::dpi::LogicalSize::new(1600.0, 950.0));
        let window = Arc::new(event_loop.create_window(attrs)?);

        // The instance is handed to Gpu, which owns it from here. Creating a
        // second instance for the device is the bug this shape prevents.
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let gpu = Gpu::with_instance(instance, Some(&surface)).await?;
        tracing::info!(adapter = %gpu.adapter_name, backend = ?gpu.backend, "GPU ready");

        let size = window.inner_size();
        let caps = surface.get_capabilities(&gpu.adapter);
        // Prefer a non-sRGB view so egui and the stage image agree about gamma.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            // wgpu 30: Auto reproduces the previous behaviour.
            color_space: wgpu::SurfaceColorSpace::Auto,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&gpu.device, &surface_config);

        let egui_ctx = egui::Context::default();
        egui_ctx.set_visuals(egui::Visuals::dark());
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            None,
            None,
            None,
        );
        let mut egui_renderer =
            egui_wgpu::Renderer::new(&gpu.device, format, egui_wgpu::RendererOptions::default());

        let textures = SourceTextures::new(&gpu);
        let renderer = Renderer::new(&gpu, &textures.layout);
        let canvas = RenderTarget::new(&gpu, Size::new(1920, 1080), "canvas");
        let target = RenderTarget::new(&gpu, Size::new(1280, 720), "viewport");
        let target_id = egui_renderer.register_native_texture(
            &gpu.device,
            &target.view,
            wgpu::FilterMode::Linear,
        );

        Ok(Self {
            last_title: app.title(),
            window,
            surface,
            surface_config,
            gpu,
            egui_ctx,
            egui_state,
            egui_renderer,
            renderer,
            textures,
            canvas,
            previz_target: None,
            backdrop_loaded: None,
            model: None,
            model_loaded: None,
            target,
            target_id,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.surface_config.width = width.max(1);
        self.surface_config.height = height.max(1);
        self.surface
            .configure(&self.gpu.device, &self.surface_config);
    }

    /// The size the offscreen stage target should be.
    /// The size to render the viewport at: the rect egui actually paints it into,
    /// in physical pixels.
    ///
    /// Not the window's size, which is what this used to be. The image is
    /// stretched into that rect, so rendering at window size squashes it by
    /// whatever the side panels take up — a previz camera at the wrong aspect,
    /// and an emulation view whose zoom does not mean what it says. Neither
    /// looks like an error; both are just geometry that is quietly wrong, and a
    /// handle dragged in a stretched view lands somewhere else again.
    fn viewport_size_hint(&self, painted_px: unmapper_core::Vec2) -> Size {
        let (w, h) = (
            self.surface_config.width.max(1),
            self.surface_config.height.max(1),
        );
        if !painted_px.is_finite() || painted_px.x < 1.0 || painted_px.y < 1.0 {
            // Before the first layout there is no painted rect to go on.
            return Size::new(w, h);
        }
        Size::new(
            (painted_px.x.round() as u32).clamp(1, w),
            (painted_px.y.round() as u32).clamp(1, h),
        )
    }

    /// Grow or shrink the canvas to match the show's virtual raster.
    ///
    /// Clamped to the device's maximum texture size: a rig wider than that
    /// cannot be held in one texture, and silently rendering a truncated canvas
    /// would lose panels off the right-hand edge without saying so.
    fn ensure_canvas(&mut self, raster: Size) {
        let limit = self.gpu.device.limits().max_texture_dimension_2d;
        let wanted = Size::new(raster.width.clamp(1, limit), raster.height.clamp(1, limit));
        if wanted != raster {
            tracing::warn!(
                "canvas {}x{} exceeds this GPU's {limit}px texture limit; clamped to {}x{}",
                raster.width,
                raster.height,
                wanted.width,
                wanted.height
            );
        }
        if self.canvas.size == wanted {
            return;
        }
        self.canvas = RenderTarget::new(&self.gpu, wanted, "canvas");
    }

    /// Read the backdrop image, if the show now names a different one.
    ///
    /// Loading is keyed on the path, so the file is read once rather than every
    /// frame; a failure is returned to the caller, which drops the path so a
    /// broken file is not retried at 60Hz.
    fn sync_backdrop(&mut self, show: &unmapper_core::Show) -> Result<()> {
        let wanted = show.geometry.backdrop.as_ref().map(|b| &b.path);
        if wanted == self.backdrop_loaded.as_ref() {
            return Ok(());
        }

        let Some(path) = wanted else {
            self.backdrop_loaded = None;
            return Ok(());
        };

        let (size, data) = unmapper_render::load_image(path)?;
        self.textures.upload(
            &self.gpu,
            BACKDROP_ID,
            FrameUpload {
                width: size.width,
                height: size.height,
                stride: (size.width * 4) as usize,
                bgra: false,
                data: &data,
                sequence: 0,
            },
        );
        self.backdrop_loaded = Some(path.clone());
        tracing::info!(path = %path.display(), width = size.width, height = size.height, "backdrop loaded");
        Ok(())
    }

    /// Read the set model, if the show now names a different one.
    ///
    /// Returns a one-line summary to show the operator on a successful load,
    /// since "did my CAD file actually come in?" is the first thing they will
    /// want to know and a silent success answers it badly.
    fn sync_model(&mut self, show: &unmapper_core::Show) -> Result<Option<String>> {
        let wanted = show
            .geometry
            .model
            .as_ref()
            .map(|m| &m.path)
            .filter(|p| !p.as_os_str().is_empty());

        if wanted == self.model_loaded.as_ref() {
            return Ok(None);
        }

        let Some(path) = wanted else {
            self.model = None;
            self.model_loaded = None;
            return Ok(None);
        };

        let mesh = unmapper_render::load_gltf(path)?;
        let summary = format!(
            "Loaded {} triangle(s){}",
            mesh.triangle_count(),
            if mesh.skipped > 0 {
                format!(", {} primitive(s) skipped", mesh.skipped)
            } else {
                String::new()
            }
        );
        self.model = Some(Model::new(&self.gpu, &mesh, DEPTH_FORMAT));
        self.model_loaded = Some(path.clone());
        tracing::info!(path = %path.display(), triangles = mesh.triangle_count(), "model loaded");
        Ok(Some(summary))
    }

    /// Resize the offscreen target, re-pointing egui's texture at the new one.
    ///
    /// Forgetting the re-point leaves egui sampling a freed texture, which shows
    /// as a viewport that stops updating after the first resize.
    fn ensure_target(&mut self, size: Size) {
        if self.target.size == size {
            return;
        }
        self.target = RenderTarget::new(&self.gpu, size, "viewport");
        self.egui_renderer.update_egui_texture_from_wgpu_texture(
            &self.gpu.device,
            &self.target.view,
            wgpu::FilterMode::Linear,
            self.target_id,
        );
    }

    fn present(
        &mut self,
        shapes: Vec<egui::epaint::ClippedShape>,
        pixels_per_point: f32,
        textures_delta: egui::TexturesDelta,
    ) -> Result<()> {
        // Texture deltas are applied BEFORE the surface is acquired, and freed
        // however this function exits.
        //
        // egui sends a texture as one full allocation followed by partial
        // updates. Skipping a frame's deltas — which an early return below used
        // to do — loses the allocation, and the next partial update then panics
        // with "Tried to update a texture that has not been allocated yet".
        // Surface timeouts are routine during window setup, so that early return
        // is hit in normal operation, not just in theory.
        // egui 0.36 carries a SmallVec of deltas per id rather than one, so the
        // partial updates for a texture arrive together and must all be applied.
        for (id, deltas) in &textures_delta.set {
            for delta in deltas {
                self.egui_renderer
                    .update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
            }
        }
        let free_textures = |r: &mut egui_wgpu::Renderer| {
            for id in &textures_delta.free {
                r.free_texture(id);
            }
        };

        use wgpu::CurrentSurfaceTexture as Cur;
        let frame = match self.surface.get_current_texture() {
            Cur::Success(f) | Cur::Suboptimal(f) => f,
            // Lost or outdated is routine during a resize or a monitor change:
            // reconfigure and pick it up next frame rather than treating a
            // normal event as a failure.
            Cur::Lost | Cur::Outdated => {
                self.surface
                    .configure(&self.gpu.device, &self.surface_config);
                free_textures(&mut self.egui_renderer);
                return Ok(());
            }
            // Nothing to draw into this frame, and nothing wrong either.
            Cur::Timeout | Cur::Occluded => {
                free_textures(&mut self.egui_renderer);
                return Ok(());
            }
            Cur::Validation => {
                free_textures(&mut self.egui_renderer);
                return Err(anyhow::anyhow!("the surface rejected a frame"));
            }
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("egui"),
            });

        let paint_jobs = self.egui_ctx.tessellate(shapes, pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.surface_config.width, self.surface_config.height],
            pixels_per_point,
        };

        self.egui_renderer.update_buffers(
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &paint_jobs,
            &screen,
        );

        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.05,
                            b: 0.06,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.egui_renderer.render(&mut pass, &paint_jobs, &screen);
        }

        self.gpu.queue.submit([encoder.finish()]);
        // wgpu 30 moved present() from SurfaceTexture onto Queue.
        self.gpu.queue.present(frame);

        free_textures(&mut self.egui_renderer);
        Ok(())
    }
}
