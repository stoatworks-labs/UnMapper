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

mod state;
mod ui;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use unmapper_core::Size;
use unmapper_render::{
    build_canvas_scene, build_previz_scene, FrameUpload, Gpu, RenderTarget, Renderer,
    SourceTextures,
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
    target: RenderTarget,
    target_id: egui::TextureId,

    last_title: String,
}

#[derive(Default)]
struct Host {
    live: Option<Live>,
    app: App,
    /// Discovery blocks, so it runs on its own thread and reports back here.
    discovery: Option<std::sync::mpsc::Receiver<Vec<unmapper_ndi::SourceName>>>,
    /// Set by the File menu; acted on after the frame, since the event loop can
    /// only be told to exit from the handler.
    quit: bool,
}

impl ApplicationHandler for Host {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.live.is_some() {
            return;
        }
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

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(live) = &mut self.live else { return };

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

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Video is arriving continuously, so redraw continuously rather than
        // waiting for input.
        if let Some(live) = &self.live {
            live.window.request_redraw();
        }
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
        let app = &mut self.app;
        let full_output = live.egui_ctx.clone().run_ui(raw_input, |ui| {
            ui::menu_bar(ui, app, &mut actions);
            ui::status_bar(ui, app);
            ui::sources_panel(ui, app, &mut actions);
            ui::inspector_panel(ui, app);
            ui::viewport(ui, app, target_id, size);
        });

        live.egui_state
            .handle_platform_output(&live.window, full_output.platform_output);

        // --- stage pass -----------------------------------------------------
        live.ensure_target(live.viewport_size_hint());

        match self.app.mode {
            ViewMode::Canvas => {
                let scene = build_canvas_scene(&self.app.show, &live.textures);
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
                live.renderer.render_previz(
                    &live.gpu,
                    &live.target.view,
                    live.target.size,
                    &camera,
                    &scene,
                    &live.textures,
                );
            }
        }

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
    fn viewport_size_hint(&self) -> Size {
        Size::new(
            self.surface_config.width.max(1),
            self.surface_config.height.max(1),
        )
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
        for (id, delta) in &textures_delta.set {
            self.egui_renderer
                .update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
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
        frame.present();

        free_textures(&mut self.egui_renderer);
        Ok(())
    }
}
