//! NDI for UnMapper: discovery, receiving, and sending.
//!
//! The runtime is **loaded at run time**, never linked — see [`sys`] for why that
//! is a licensing requirement and not just a build convenience. Nothing in this
//! crate needs the SDK present to compile, and a machine with no runtime gets a
//! clear error naming the download rather than a failure to start.
//!
//! # The receive model
//!
//! `NDIlib_recv_capture_v2` blocks, so each source owns a thread. That thread
//! publishes into a single-slot mailbox that the render loop reads at its own
//! pace: **newest frame wins, and older frames are dropped**. That is the correct
//! policy here — this is a live wall, so a late frame is worthless, and queueing
//! would trade latency for a smoothness nobody watching a stage wants.
//!
//! The buffer is handed back and forth rather than reallocated, so a steady
//! stream at a steady resolution settles into two buffers and stops allocating.

pub mod sys;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use parking_lot::Mutex;

pub use sys::{SourceName, REDIST_URL};

/// How a received frame's bytes are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 4 bytes per pixel, red first. What UnMapper asks for.
    Rgba,
    /// 4 bytes per pixel, blue first. Some senders deliver this regardless.
    Bgra,
    /// Anything else the SDK handed over — carried so it can be reported rather
    /// than rendered as garbage.
    Unsupported(u32),
}

impl PixelFormat {
    fn from_four_cc(cc: u32) -> Self {
        match cc {
            // RGBX and BGRX are the opaque variants; the X byte is simply ignored
            // rather than being a different layout.
            sys::FOURCC_RGBA | sys::FOURCC_RGBX => PixelFormat::Rgba,
            sys::FOURCC_BGRA | sys::FOURCC_BGRX => PixelFormat::Bgra,
            other => PixelFormat::Unsupported(other),
        }
    }

    /// The four-character code as text, for error messages.
    pub fn label(&self) -> String {
        match self {
            PixelFormat::Rgba => "RGBA".into(),
            PixelFormat::Bgra => "BGRA".into(),
            PixelFormat::Unsupported(cc) => String::from_utf8_lossy(&cc.to_le_bytes()).into_owned(),
        }
    }
}

/// One received frame, owned.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Bytes per row, which is **not** always `width * 4` — senders pad.
    pub stride: usize,
    pub format: PixelFormat,
    pub data: Vec<u8>,
    /// Increments once per frame published, so a consumer can tell a new frame
    /// from the one it already uploaded without comparing pixels.
    pub sequence: u64,
}

impl Frame {
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// What a receiver is currently doing, for the UI.
#[derive(Debug, Clone, Default)]
pub struct ReceiverStatus {
    pub connected: bool,
    pub frames: u64,
    /// Frames the render loop never saw, because a newer one replaced them.
    /// Entirely normal — it is the source running faster than the display.
    pub dropped: u64,
    pub width: u32,
    pub height: u32,
    pub format: Option<String>,
    pub fps: f32,
    pub last_error: Option<String>,
}

/// The loaded NDI runtime.
#[derive(Clone)]
pub struct Ndi {
    api: Arc<sys::Api>,
}

impl Ndi {
    /// Load the runtime, or explain why it could not be found.
    pub fn load() -> Result<Self> {
        Ok(Self { api: sys::load()? })
    }

    pub fn version(&self) -> &str {
        &self.api.version
    }

    pub fn library_path(&self) -> &std::path::Path {
        &self.api.path
    }

    /// Look for senders on the network for up to `timeout`.
    ///
    /// Discovery is not instant — mDNS takes a moment to answer — so a single
    /// call with a short timeout can legitimately return nothing on a network
    /// that has sources. The UI should re-scan rather than conclude there are none.
    pub fn discover(&self, timeout: Duration) -> Result<Vec<SourceName>> {
        let mut finder = sys::Finder::new(self.api.clone(), true)?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            finder.wait_for_sources(remaining.as_millis().min(u32::MAX as u128) as u32);
            if Instant::now() >= deadline {
                break;
            }
        }
        let mut sources = finder.sources();
        sources.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(sources)
    }

    /// Connect to a source and start receiving on a background thread.
    pub fn receive(&self, source: &str, recv_name: &str) -> ReceiverHandle {
        ReceiverHandle::spawn(self.api.clone(), source.to_owned(), recv_name.to_owned())
    }

    /// Create a sender.
    pub fn sender(&self, name: &str) -> Result<Sender> {
        Ok(Sender {
            inner: sys::Sender::new(self.api.clone(), name)?,
        })
    }
}

/// The mailbox between the receive thread and the render loop.
struct Slot {
    frame: Option<Frame>,
    /// A buffer handed back by the consumer, to be filled again.
    spare: Option<Vec<u8>>,
    status: ReceiverStatus,
}

/// A running receiver. Dropping it stops the thread.
pub struct ReceiverHandle {
    slot: Arc<Mutex<Slot>>,
    running: Arc<AtomicBool>,
    sequence: Arc<AtomicU64>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub source: String,
}

impl ReceiverHandle {
    fn spawn(api: Arc<sys::Api>, source: String, recv_name: String) -> Self {
        let slot = Arc::new(Mutex::new(Slot {
            frame: None,
            spare: None,
            status: ReceiverStatus::default(),
        }));
        let running = Arc::new(AtomicBool::new(true));
        let sequence = Arc::new(AtomicU64::new(0));

        let thread = {
            let slot = slot.clone();
            let running = running.clone();
            let sequence = sequence.clone();
            let source = source.clone();
            std::thread::Builder::new()
                .name(format!("ndi-recv {source}"))
                .spawn(move || receive_loop(api, source, recv_name, slot, running, sequence))
                .expect("spawning a receive thread")
        };

        Self {
            slot,
            running,
            sequence,
            thread: Some(thread),
            source,
        }
    }

    /// The newest frame, if one has arrived since the last call.
    ///
    /// Takes the frame out of the mailbox, so a second call with no new frame in
    /// between returns `None` — the caller is expected to keep its own texture
    /// rather than re-upload the same pixels.
    pub fn take_frame(&self) -> Option<Frame> {
        self.slot.lock().frame.take()
    }

    /// Hand a consumed frame's buffer back so it can be filled again instead of
    /// reallocated. Optional — skipping it costs an allocation, not correctness.
    pub fn recycle(&self, buffer: Vec<u8>) {
        let mut slot = self.slot.lock();
        if slot.spare.is_none() {
            slot.spare = Some(buffer);
        }
    }

    /// The sequence number of the most recently published frame.
    pub fn sequence(&self) -> u64 {
        self.sequence.load(Ordering::Relaxed)
    }

    pub fn status(&self) -> ReceiverStatus {
        self.slot.lock().status.clone()
    }
}

impl Drop for ReceiverHandle {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            // The loop's capture timeout bounds how long this waits.
            let _ = t.join();
        }
    }
}

/// How long to block in `capture` before checking whether we have been asked to
/// stop. Short enough that shutdown is not noticeable, long enough not to spin.
const CAPTURE_TIMEOUT_MS: u32 = 100;

fn receive_loop(
    api: Arc<sys::Api>,
    source: String,
    recv_name: String,
    slot: Arc<Mutex<Slot>>,
    running: Arc<AtomicBool>,
    sequence: Arc<AtomicU64>,
) {
    let mut receiver = match sys::Receiver::new(api, &source, &recv_name, false) {
        Ok(r) => r,
        Err(e) => {
            slot.lock().status.last_error = Some(e.to_string());
            tracing::warn!(%source, error = %e, "could not create NDI receiver");
            return;
        }
    };

    let mut fps_window_start = Instant::now();
    let mut fps_window_frames = 0u32;

    while running.load(Ordering::Relaxed) {
        let mut idle = false;
        match receiver.capture(CAPTURE_TIMEOUT_MS) {
            sys::Captured::Video(video) => {
                let format = PixelFormat::from_four_cc(video.four_cc());
                let (width, height, stride) = (video.width(), video.height(), video.stride());

                let Some(src) = video.data() else {
                    continue;
                };

                if let PixelFormat::Unsupported(_) = format {
                    let mut guard = slot.lock();
                    guard.status.last_error = Some(format!(
                        "source is sending {}, which UnMapper cannot display",
                        format.label()
                    ));
                    guard.status.format = Some(format.label());
                    continue;
                }

                let seq = sequence.fetch_add(1, Ordering::Relaxed) + 1;

                let mut guard = slot.lock();
                // Reuse a buffer where possible: the spare handed back by the
                // consumer first, then the buffer of a frame that was never
                // collected. Both are the right size in the steady state.
                let mut buffer = guard
                    .spare
                    .take()
                    .or_else(|| guard.frame.take().map(|f| f.data))
                    .unwrap_or_default();
                buffer.clear();
                buffer.extend_from_slice(src);

                if guard.frame.is_some() {
                    guard.status.dropped += 1;
                }
                guard.status.frames += 1;
                guard.status.connected = true;
                guard.status.width = width;
                guard.status.height = height;
                guard.status.format = Some(format.label());
                guard.status.last_error = None;
                guard.frame = Some(Frame {
                    width,
                    height,
                    stride,
                    format,
                    data: buffer,
                    sequence: seq,
                });
                drop(guard);

                fps_window_frames += 1;
                let elapsed = fps_window_start.elapsed();
                if elapsed >= Duration::from_secs(1) {
                    let fps = fps_window_frames as f32 / elapsed.as_secs_f32();
                    slot.lock().status.fps = fps;
                    fps_window_start = Instant::now();
                    fps_window_frames = 0;
                }
            }
            sys::Captured::Other => {}
            // Handled after the match: `Captured` borrows the receiver for as
            // long as it is alive, so asking it anything here would overlap the
            // borrow `capture` still holds.
            sys::Captured::Nothing => idle = true,
        }

        if idle {
            // A silent source is not an error, but it is worth showing.
            let connected = receiver.is_connected();
            let mut guard = slot.lock();
            guard.status.connected = connected;
            if !connected {
                guard.status.fps = 0.0;
            }
        }
    }
}

/// An NDI output.
pub struct Sender {
    inner: sys::Sender,
}

impl Sender {
    /// Publish one RGBA frame.
    ///
    /// `data` must hold `height * stride` bytes. The SDK copies before returning,
    /// so the caller's buffer is free immediately afterwards.
    pub fn send_rgba(
        &mut self,
        width: u32,
        height: u32,
        stride: usize,
        data: &[u8],
        fps: (i32, i32),
    ) {
        debug_assert!(data.len() >= height as usize * stride);
        let frame = sys::VideoFrameV2 {
            xres: width as i32,
            yres: height as i32,
            four_cc: sys::FOURCC_RGBA,
            frame_rate_n: fps.0,
            frame_rate_d: fps.1,
            picture_aspect_ratio: 0.0, // 0 means "derive from the resolution"
            frame_format_type: sys::FRAME_FORMAT_PROGRESSIVE,
            timecode: sys::SEND_TIMECODE_SYNTHESIZE,
            p_data: data.as_ptr() as *mut u8,
            line_stride_in_bytes: stride as i32,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        };
        self.inner.send_video(&frame);
    }

    /// How many receivers are watching, or `None` on a runtime that cannot say.
    pub fn connections(&mut self) -> Option<i32> {
        self.inner.connections(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_cc_maps_the_x_variants_onto_their_alpha_equivalents() {
        assert_eq!(
            PixelFormat::from_four_cc(sys::FOURCC_RGBA),
            PixelFormat::Rgba
        );
        assert_eq!(
            PixelFormat::from_four_cc(sys::FOURCC_RGBX),
            PixelFormat::Rgba
        );
        assert_eq!(
            PixelFormat::from_four_cc(sys::FOURCC_BGRA),
            PixelFormat::Bgra
        );
        assert_eq!(
            PixelFormat::from_four_cc(sys::FOURCC_BGRX),
            PixelFormat::Bgra
        );
    }

    #[test]
    fn an_unknown_four_cc_is_reported_by_name_rather_than_guessed_at() {
        let uyvy = PixelFormat::from_four_cc(sys::FOURCC_UYVY);
        assert_eq!(uyvy, PixelFormat::Unsupported(sys::FOURCC_UYVY));
        // The label is what an operator sees, so it has to read as the fourcc
        // they would recognise from the sender's settings.
        assert_eq!(uyvy.label(), "UYVY");
    }

    #[test]
    fn a_frame_with_no_pixels_is_recognised_as_empty() {
        let f = Frame {
            width: 0,
            height: 0,
            stride: 0,
            format: PixelFormat::Rgba,
            data: vec![],
            sequence: 1,
        };
        assert!(f.is_empty());
    }
}
