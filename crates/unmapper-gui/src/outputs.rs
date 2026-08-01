//! Output windows — the monitors standing in for the wall.
//!
//! Each enabled [`OutputTarget::Display`] gets its own window and its own
//! surface, all on the one device the main window already uses. The emulation
//! canvas is rendered **once** per frame at full resolution; every output then
//! blits the region it stands in for. So ten monitors cost one render and ten
//! blits, and every one of them is showing the same frame.

use std::collections::HashMap;
use std::sync::Arc;

use unmapper_core::{OutputTarget, OutputView, Show, Size};
use unmapper_render::{Blit, Gpu, RenderTarget, TARGET_FORMAT};
use winit::event_loop::ActiveEventLoop;
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window, WindowId};

/// A monitor, as the UI needs to describe it.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub name: String,
    pub size: Size,
    pub scale: f64,
}

impl MonitorInfo {
    pub fn label(&self, index: usize) -> String {
        format!(
            "{index}: {} ({}×{})",
            self.name, self.size.width, self.size.height
        )
    }
}

pub fn enumerate_monitors(event_loop: &ActiveEventLoop) -> Vec<MonitorInfo> {
    event_loop
        .available_monitors()
        .map(|m| MonitorInfo {
            name: m.name().unwrap_or_else(|| "display".into()),
            size: Size::new(m.size().width, m.size().height),
            scale: m.scale_factor(),
        })
        .collect()
}

/// What `sync` decided one window should be, on its way to `create`.
struct WantedWindow<'a> {
    output_id: &'a str,
    monitor: Option<MonitorHandle>,
    fullscreen: bool,
    previz_size: Option<Size>,
}

/// One output's window and surface.
struct OutputWindow {
    output_id: String,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// Which monitor index and fullscreen state this window was built for, so a
    /// change in the show can be detected and the window rebuilt.
    monitor: usize,
    fullscreen: bool,
    /// `Some` when this output shows the previz camera rather than a canvas crop.
    previz_size: Option<Size>,
}

/// An NDI output: the canvas region cropped into an offscreen target, read back,
/// and published as an NDI source.
///
/// Unlike a display output this costs a **GPU readback every frame**, which is a
/// real stall — there is no way around it while the SDK takes CPU pixels. That is
/// the reason NDI outputs are opt-in per output rather than always running.
struct NdiOutput {
    output_id: String,
    sender: unmapper_ndi::Sender,
    target: RenderTarget,
    name: String,
    previz_size: Option<Size>,
}

/// Every output window, plus the blit pipelines that feed them.
#[derive(Default)]
pub struct OutputWindows {
    windows: Vec<OutputWindow>,
    ndi: Vec<NdiOutput>,
    /// One blit pipeline per surface format. Different monitors can prefer
    /// different formats, and a pipeline is bound to the format it writes.
    blits: HashMap<wgpu::TextureFormat, Blit>,
    /// The bind group for the canvas texture, rebuilt when the canvas is
    /// recreated (which is why it is keyed on the canvas's size).
    canvas_bind: Option<(Size, wgpu::TextureFormat, wgpu::BindGroup)>,
    /// Same, for the previz target. A previz output cannot crop the canvas — a
    /// camera view has to be rendered on its own — so it has its own source.
    previz_bind: Option<(Size, wgpu::BindGroup)>,
    /// Reported once per failing output rather than every frame.
    reported_errors: HashMap<String, String>,
}

impl OutputWindows {
    /// Reconcile NDI outputs against the show. Cheap when nothing changed.
    pub fn sync_ndi(
        &mut self,
        gpu: &Gpu,
        show: &Show,
        ndi: Option<&unmapper_ndi::Ndi>,
    ) -> Vec<String> {
        let mut messages = Vec::new();

        let wanted: Vec<(&str, &str, Size, Option<Size>)> = show
            .outputs
            .iter()
            .filter(|o| o.enabled)
            .filter_map(|o| match (&o.target, &o.view) {
                (OutputTarget::Ndi { name }, OutputView::Emulation { region })
                    if !name.is_empty() =>
                {
                    Some((
                        o.id.as_str(),
                        name.as_str(),
                        Size::new(region.width.max(1.0) as u32, region.height.max(1.0) as u32),
                        None,
                    ))
                }
                (OutputTarget::Ndi { name }, OutputView::Previz { .. }) if !name.is_empty() => {
                    let size = Size::new(o.size.width.max(16), o.size.height.max(16));
                    Some((o.id.as_str(), name.as_str(), size, Some(size)))
                }
                _ => None,
            })
            .collect();

        self.ndi.retain(|n| {
            wanted.iter().any(|(id, name, size, previz)| {
                *id == n.output_id
                    && *name == n.name
                    && *size == n.target.size
                    && *previz == n.previz_size
            })
        });

        for (id, name, size, previz_size) in wanted {
            if self.ndi.iter().any(|n| n.output_id == id) {
                continue;
            }
            let Some(ndi) = ndi else {
                let msg = format!("output \"{id}\" is NDI, but no NDI runtime is loaded");
                if self.reported_errors.get(id) != Some(&msg) {
                    self.reported_errors.insert(id.to_owned(), msg.clone());
                    messages.push(msg);
                }
                continue;
            };
            match ndi.sender(name) {
                Ok(sender) => {
                    tracing::info!(output = %id, %name, "NDI output opened");
                    self.reported_errors.remove(id);
                    self.ndi.push(NdiOutput {
                        output_id: id.to_owned(),
                        sender,
                        target: RenderTarget::new(gpu, size, "ndi output"),
                        name: name.to_owned(),
                        previz_size,
                    });
                }
                Err(e) => {
                    let msg = format!("output \"{id}\" could not start NDI: {e}");
                    if self.reported_errors.get(id) != Some(&msg) {
                        self.reported_errors.insert(id.to_owned(), msg.clone());
                        messages.push(msg);
                    }
                }
            }
        }
        messages
    }

    /// Whether anything needs the emulation canvas rendered this frame.
    pub fn needs_canvas(&self) -> bool {
        self.windows.iter().any(|w| w.previz_size.is_none())
            || self.ndi.iter().any(|n| n.previz_size.is_none())
    }

    /// The size a previz output wants, if any wants one. The largest wins, so a
    /// single render serves every previz output.
    pub fn previz_size(&self) -> Option<Size> {
        self.windows
            .iter()
            .filter_map(|w| w.previz_size)
            .chain(self.ndi.iter().filter_map(|n| n.previz_size))
            .reduce(|a, b| Size::new(a.width.max(b.width), a.height.max(b.height)))
    }

    pub fn ndi_count(&self) -> usize {
        self.ndi.len()
    }

    /// How many output windows are actually open — which is not the same as how
    /// many the show declares, since one can fail to open or be closed by hand.
    pub fn open_count(&self) -> usize {
        self.windows.len()
    }

    /// Whether `id` belongs to one of these windows.
    pub fn owns(&self, id: WindowId) -> bool {
        self.windows.iter().any(|w| w.window.id() == id)
    }

    /// Close the window with this id, returning the output it was showing.
    ///
    /// Closing an output window is a request to stop that output, not to quit —
    /// the alternative, ignoring the close button, leaves a fullscreen window
    /// that cannot be dismissed.
    pub fn close(&mut self, id: WindowId) -> Option<String> {
        let index = self.windows.iter().position(|w| w.window.id() == id)?;
        Some(self.windows.remove(index).output_id)
    }

    pub fn resize(&mut self, gpu: &Gpu, id: WindowId, width: u32, height: u32) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.window.id() == id) {
            w.config.width = width.max(1);
            w.config.height = height.max(1);
            w.surface.configure(&gpu.device, &w.config);
        }
    }

    pub fn request_redraw(&self) {
        for w in &self.windows {
            w.window.request_redraw();
        }
    }

    /// Create, destroy and rebuild windows so they match the show.
    ///
    /// Returns any error worth showing the operator, once per output rather than
    /// once per frame.
    pub fn sync(
        &mut self,
        event_loop: &ActiveEventLoop,
        gpu: &Gpu,
        show: &Show,
        monitors: &[MonitorInfo],
    ) -> Vec<String> {
        let mut messages = Vec::new();

        let wanted: Vec<(&str, usize, bool, Option<Size>)> = show
            .outputs
            .iter()
            .filter(|o| o.enabled)
            .filter_map(|o| match &o.target {
                OutputTarget::Display { index, fullscreen } => Some((
                    o.id.as_str(),
                    *index,
                    *fullscreen,
                    match &o.view {
                        OutputView::Previz { .. } => {
                            Some(Size::new(o.size.width.max(16), o.size.height.max(16)))
                        }
                        OutputView::Emulation { .. } => None,
                    },
                )),
                _ => None,
            })
            .collect();

        // Drop windows that are no longer wanted, or whose monitor/fullscreen
        // changed — those need rebuilding rather than mutating, because the
        // monitor a window is on is fixed at creation.
        self.windows.retain(|w| {
            wanted.iter().any(|(id, m, fs, pv)| {
                *id == w.output_id && *m == w.monitor && *fs == w.fullscreen && *pv == w.previz_size
            })
        });

        for (id, monitor_index, fullscreen, previz_size) in wanted {
            if self.windows.iter().any(|w| w.output_id == id) {
                continue;
            }

            let monitor: Option<MonitorHandle> = event_loop.available_monitors().nth(monitor_index);
            if monitor.is_none() && !monitors.is_empty() {
                let msg = format!(
                    "output \"{id}\" wants display {monitor_index}, but only {} are connected",
                    monitors.len()
                );
                if self.reported_errors.get(id) != Some(&msg) {
                    self.reported_errors.insert(id.to_owned(), msg.clone());
                    messages.push(msg);
                }
                continue;
            }

            match self.create(
                event_loop,
                gpu,
                WantedWindow {
                    output_id: id,
                    monitor,
                    fullscreen,
                    previz_size,
                },
                show,
            ) {
                Ok(window) => {
                    self.reported_errors.remove(id);
                    self.windows.push(window);
                }
                Err(e) => {
                    let msg = format!("output \"{id}\" could not open: {e}");
                    if self.reported_errors.get(id) != Some(&msg) {
                        self.reported_errors.insert(id.to_owned(), msg.clone());
                        messages.push(msg);
                    }
                }
            }
        }

        messages
    }

    fn create(
        &mut self,
        event_loop: &ActiveEventLoop,
        gpu: &Gpu,
        wanted: WantedWindow<'_>,
        show: &Show,
    ) -> anyhow::Result<OutputWindow> {
        let WantedWindow {
            output_id,
            monitor,
            fullscreen,
            previz_size,
        } = wanted;
        let output = show
            .outputs
            .iter()
            .find(|o| o.id == output_id)
            .ok_or_else(|| anyhow::anyhow!("no such output"))?;

        // A windowed output opens at the region's own size, so one canvas pixel
        // is one screen pixel without the operator resizing anything.
        let (w, h) = match &output.view {
            OutputView::Emulation { region } => (
                region.width.max(16.0) as u32,
                region.height.max(16.0) as u32,
            ),
            OutputView::Previz { .. } => (output.size.width.max(16), output.size.height.max(16)),
        };

        let mut attrs = Window::default_attributes()
            .with_title(format!("{} — UnMapper output", output.name))
            .with_inner_size(winit::dpi::PhysicalSize::new(w, h));

        if fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(monitor.clone())));
        } else if let Some(m) = &monitor {
            // Place a windowed output on its monitor even though it is not
            // fullscreen, so "display 2" still means display 2 — and cascade
            // them, because two outputs on one monitor otherwise open exactly on
            // top of each other and look like one window that failed to appear.
            let p = m.position();
            let step = 48 * self.windows.len() as i32;
            attrs = attrs.with_position(winit::dpi::PhysicalPosition::new(
                p.x + 40 + step,
                p.y + 40 + step,
            ));
        }

        let window = Arc::new(event_loop.create_window(attrs)?);
        let surface = gpu.instance.create_surface(window.clone())?;

        let caps = surface.get_capabilities(&gpu.adapter);
        // Non-sRGB where possible: the canvas holds exactly what Resolume sent,
        // and an sRGB surface would re-encode it on the way out.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&gpu.device, &config);

        self.blits
            .entry(format)
            .or_insert_with(|| Blit::new(gpu, format));

        tracing::info!(output = %output_id, ?format, fullscreen, "output window opened");

        Ok(OutputWindow {
            output_id: output_id.to_owned(),
            window,
            surface,
            config,
            monitor: match &output.target {
                OutputTarget::Display { index, .. } => *index,
                _ => 0,
            },
            fullscreen,
            previz_size,
        })
    }

    /// Blit each output's region of the canvas to its window, and publish any
    /// NDI outputs.
    pub fn render(
        &mut self,
        gpu: &Gpu,
        canvas: &RenderTarget,
        previz: Option<&RenderTarget>,
        show: &Show,
    ) {
        if self.windows.is_empty() && self.ndi.is_empty() {
            return;
        }

        // NDI needs a blit pipeline for the offscreen format even when no window
        // exists to have created one.
        if !self.ndi.is_empty() {
            self.blits
                .entry(TARGET_FORMAT)
                .or_insert_with(|| Blit::new(gpu, TARGET_FORMAT));
        }

        // One bind group for the canvas, shared by every output and rebuilt only
        // when the canvas texture itself is replaced.
        let need_rebuild = match &self.canvas_bind {
            Some((size, _, _)) => *size != canvas.size,
            None => true,
        };
        if need_rebuild {
            // Any blit will do to build the bind group — the source layout is
            // the same for all of them.
            let Some(any) = self.blits.values().next() else {
                return;
            };
            self.canvas_bind = Some((canvas.size, any.format(), any.source(gpu, &canvas.view)));
        }
        // Moved out for the duration of the frame: the NDI loop needs `&mut
        // self` for its senders, which would otherwise conflict with a borrow of
        // `self.canvas_bind`. Put back before returning.
        if let Some(previz) = previz {
            let stale = !matches!(&self.previz_bind, Some((s, _)) if *s == previz.size);
            if stale {
                if let Some(any) = self.blits.values().next() {
                    self.previz_bind = Some((previz.size, any.source(gpu, &previz.view)));
                }
            }
        }

        let taken = self.canvas_bind.take();
        let Some((size, format, bind)) = taken else {
            return;
        };

        for w in &self.windows {
            let Some(output) = show.outputs.iter().find(|o| o.id == w.output_id) else {
                continue;
            };
            // A previz output draws the whole camera view; an emulation output
            // crops the canvas. Different source texture, same blit.
            let (source, source_size, region) = match &output.view {
                OutputView::Emulation { region } => (&bind, canvas.size, *region),
                OutputView::Previz { .. } => {
                    let Some((size, b)) = &self.previz_bind else {
                        continue;
                    };
                    (
                        b,
                        *size,
                        unmapper_core::Rect::new(0.0, 0.0, size.width as f32, size.height as f32),
                    )
                }
            };
            let Some(blit) = self.blits.get(&w.config.format) else {
                continue;
            };

            use wgpu::CurrentSurfaceTexture as Cur;
            let frame = match w.surface.get_current_texture() {
                Cur::Success(f) | Cur::Suboptimal(f) => f,
                Cur::Lost | Cur::Outdated => {
                    w.surface.configure(&gpu.device, &w.config);
                    continue;
                }
                Cur::Timeout | Cur::Occluded => continue,
                Cur::Validation => continue,
            };

            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("output"),
                });
            blit.draw(gpu, &mut encoder, &view, source, source_size, region);
            gpu.queue.submit([encoder.finish()]);
            frame.present();
        }

        let previz_bind = self.previz_bind.take();
        self.publish_ndi(gpu, canvas, show, &bind, previz_bind.as_ref());
        self.previz_bind = previz_bind;
        self.canvas_bind = Some((size, format, bind));
    }

    fn publish_ndi(
        &mut self,
        gpu: &Gpu,
        canvas: &RenderTarget,
        show: &Show,
        bind: &wgpu::BindGroup,
        previz_bind: Option<&(Size, wgpu::BindGroup)>,
    ) {
        let Some(blit) = self.blits.get(&TARGET_FORMAT) else {
            return;
        };

        for out in &mut self.ndi {
            let Some(output) = show.outputs.iter().find(|o| o.id == out.output_id) else {
                continue;
            };
            let (source, source_size, region) = match &output.view {
                OutputView::Emulation { region } => (bind, canvas.size, *region),
                OutputView::Previz { .. } => {
                    let Some((size, b)) = previz_bind else {
                        continue;
                    };
                    (
                        b,
                        *size,
                        unmapper_core::Rect::new(0.0, 0.0, size.width as f32, size.height as f32),
                    )
                }
            };

            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("ndi output"),
                });
            blit.draw(
                gpu,
                &mut encoder,
                &out.target.view,
                source,
                source_size,
                region,
            );
            gpu.queue.submit([encoder.finish()]);

            // The readback stalls until the GPU has finished, which is why this
            // is the last thing in the frame.
            let pixels = out.target.read_rgba(gpu);
            let stride = (out.target.size.width * 4) as usize;
            out.sender.send_rgba(
                out.target.size.width,
                out.target.size.height,
                stride,
                &pixels,
                // The render loop paces frames; this is a declared rate, not a
                // clock the SDK should enforce.
                (60, 1),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_monitor_label_names_its_index_and_size() {
        let m = MonitorInfo {
            name: "Studio Display".into(),
            size: Size::new(5120, 2880),
            scale: 2.0,
        };
        assert_eq!(m.label(1), "1: Studio Display (5120×2880)");
    }
}
