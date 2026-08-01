//! Minimal dynamic binding to the NDI runtime — no SDK needed at build time.
//!
//! Ported from `openstage`'s `native-node/src/ndi_sys.rs`, which binds discovery
//! and **sending**. UnMapper's whole reason to exist is *receiving*, so the
//! `NDIlib_recv_*` half is added here.
//!
//! # Why loaded rather than linked
//!
//! The NDI licence permits redistribution, but only if the licence you ship under
//! forbids modifying, reverse-engineering and decompiling the SDK. UnMapper is
//! MIT, which grants exactly those rights, so it cannot also forbid them — the
//! two cannot both be true in one source tree. Loading at run time sidesteps the
//! question entirely: no NDI code is distributed, only the flat C ABI is named,
//! and a machine without a runtime still builds and runs with NDI sources simply
//! unavailable.
//!
//! It also removes the build-time dependency, which is what makes cross-compiled
//! release binaries possible at all. The canonical write-up of all this lives in
//! the sibling `weblinked/docs/06-ndi-distribution.md`.
//!
//! # Why flat symbols
//!
//! `NDIlib_recv_create_v3` and friends are exported by every NDI 5 and 6 runtime.
//! `NDIlib_v6_load()` returns a versioned struct whose layout changes between SDK
//! generations, so binding that would refuse a v5 runtime for no good reason.
//!
//! # Struct layouts
//!
//! The types below mirror `Processing.NDI.structs.h`, `.Recv.h`, `.Send.h` and
//! `.Find.h` field for field, in declaration order, as `#[repr(C)]`. The tests at
//! the bottom of this file are the only thing standing between a layout typo and
//! silent memory corruption — **do not change a struct without running them**.

#![allow(dead_code)]

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};

/// Where to send an operator with no runtime. Platform-specific, and empty on
/// Linux because no one-click redistributable exists there.
pub const REDIST_URL: &str = if cfg!(target_os = "macos") {
    "http://ndi.link/NDIRedistV6Apple"
} else if cfg!(target_os = "windows") {
    "http://ndi.link/NDIRedistV6"
} else {
    "https://ndi.video/for-developers/ndi-sdk/"
};

const REDIST_FOLDER_VAR: &str = "NDI_RUNTIME_DIR_V6";
/// Checked after V6 so a machine with both prefers the newer runtime.
const REDIST_FOLDER_VAR_V5: &str = "NDI_RUNTIME_DIR_V5";

#[cfg(target_os = "macos")]
const LIBRARY_NAMES: &[&str] = &["libndi.dylib"];
#[cfg(target_os = "windows")]
const LIBRARY_NAMES: &[&str] = &["Processing.NDI.Lib.x64.dll"];
#[cfg(all(unix, not(target_os = "macos")))]
const LIBRARY_NAMES: &[&str] = &["libndi.so.6", "libndi.so.5", "libndi.so"];

#[cfg(target_os = "macos")]
const EXTRA_DIRS: &[&str] = &[
    "/Library/NDI SDK for Apple/lib/macOS",
    "/Library/NDI SDK for macOS/lib/macOS",
    "/usr/local/lib",
    "/opt/homebrew/lib",
];
#[cfg(target_os = "windows")]
const EXTRA_DIRS: &[&str] = &[];
#[cfg(all(unix, not(target_os = "macos")))]
const EXTRA_DIRS: &[&str] = &["/usr/local/lib", "/usr/lib"];

pub const FOURCC_RGBA: u32 = u32::from_le_bytes(*b"RGBA");
pub const FOURCC_RGBX: u32 = u32::from_le_bytes(*b"RGBX");
pub const FOURCC_BGRA: u32 = u32::from_le_bytes(*b"BGRA");
pub const FOURCC_BGRX: u32 = u32::from_le_bytes(*b"BGRX");
pub const FOURCC_UYVY: u32 = u32::from_le_bytes(*b"UYVY");

pub const FRAME_FORMAT_PROGRESSIVE: c_int = 1;
pub const SEND_TIMECODE_SYNTHESIZE: i64 = i64::MAX;

/// `NDIlib_frame_type_e`.
pub const FRAME_TYPE_NONE: c_int = 0;
pub const FRAME_TYPE_VIDEO: c_int = 1;
pub const FRAME_TYPE_AUDIO: c_int = 2;
pub const FRAME_TYPE_METADATA: c_int = 3;
pub const FRAME_TYPE_ERROR: c_int = 4;
pub const FRAME_TYPE_STATUS_CHANGE: c_int = 100;

/// `NDIlib_recv_color_format_e`.
///
/// UnMapper asks for `RGBX_RGBA` so frames arrive in a layout wgpu can upload
/// directly. The alternative, `fastest`, delivers UYVY — half the bytes over the
/// wire and noticeably cheaper at 4K, but it needs a YUV→RGB conversion in the
/// shader and carries Rec.601/709 matrix questions with it. Correct first; the
/// UYVY path is a real optimisation to make later, not a thing already done.
pub const RECV_COLOR_RGBX_RGBA: c_int = 2;
pub const RECV_COLOR_FASTEST: c_int = 100;
pub const RECV_COLOR_BEST: c_int = 101;

/// `NDIlib_recv_bandwidth_e`.
pub const RECV_BANDWIDTH_METADATA_ONLY: c_int = -10;
pub const RECV_BANDWIDTH_AUDIO_ONLY: c_int = 10;
pub const RECV_BANDWIDTH_LOWEST: c_int = 0;
pub const RECV_BANDWIDTH_HIGHEST: c_int = 100;

#[repr(C)]
pub struct SendCreate {
    pub p_ndi_name: *const c_char,
    pub p_groups: *const c_char,
    pub clock_video: bool,
    pub clock_audio: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VideoFrameV2 {
    pub xres: c_int,
    pub yres: c_int,
    pub four_cc: u32,
    pub frame_rate_n: c_int,
    pub frame_rate_d: c_int,
    pub picture_aspect_ratio: f32,
    pub frame_format_type: c_int,
    pub timecode: i64,
    pub p_data: *mut u8,
    /// Union in C (`line_stride_in_bytes` / `data_size_in_bytes`); both arms are
    /// `int`, so one field models it exactly.
    pub line_stride_in_bytes: c_int,
    pub p_metadata: *const c_char,
    pub timestamp: i64,
}

impl Default for VideoFrameV2 {
    fn default() -> Self {
        Self {
            xres: 0,
            yres: 0,
            four_cc: FOURCC_RGBA,
            frame_rate_n: 30000,
            frame_rate_d: 1001,
            picture_aspect_ratio: 0.0,
            frame_format_type: FRAME_FORMAT_PROGRESSIVE,
            timecode: 0,
            p_data: std::ptr::null_mut(),
            line_stride_in_bytes: 0,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        }
    }
}

// SAFETY: a plain C frame header. Only ever paired with a buffer whose ownership
// is tracked alongside it.
unsafe impl Send for VideoFrameV2 {}

#[repr(C)]
pub struct FindCreate {
    pub show_local_sources: bool,
    pub p_groups: *const c_char,
    pub p_extra_ips: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SourceRaw {
    pub p_ndi_name: *const c_char,
    /// Union in C (`p_url_address` / the deprecated `p_ip_address`), both
    /// `const char*`.
    pub p_url_address: *const c_char,
}

impl Default for SourceRaw {
    fn default() -> Self {
        Self {
            p_ndi_name: std::ptr::null(),
            p_url_address: std::ptr::null(),
        }
    }
}

/// `NDIlib_recv_create_v3_t`.
#[repr(C)]
pub struct RecvCreateV3 {
    pub source_to_connect_to: SourceRaw,
    pub color_format: c_int,
    pub bandwidth: c_int,
    pub allow_video_fields: bool,
    pub p_ndi_recv_name: *const c_char,
}

type FnInitialize = unsafe extern "C" fn() -> bool;
type FnVersion = unsafe extern "C" fn() -> *const c_char;
type FnSendCreate = unsafe extern "C" fn(*const SendCreate) -> *mut c_void;
type FnSendDestroy = unsafe extern "C" fn(*mut c_void);
type FnSendVideoV2 = unsafe extern "C" fn(*mut c_void, *const VideoFrameV2);
type FnSendGetNoConnections = unsafe extern "C" fn(*mut c_void, u32) -> c_int;
type FnFindCreateV2 = unsafe extern "C" fn(*const FindCreate) -> *mut c_void;
type FnFindDestroy = unsafe extern "C" fn(*mut c_void);
type FnFindGetCurrentSources = unsafe extern "C" fn(*mut c_void, *mut u32) -> *const SourceRaw;
type FnFindWaitForSources = unsafe extern "C" fn(*mut c_void, u32) -> bool;
type FnRecvCreateV3 = unsafe extern "C" fn(*const RecvCreateV3) -> *mut c_void;
type FnRecvDestroy = unsafe extern "C" fn(*mut c_void);
type FnRecvCaptureV2 =
    unsafe extern "C" fn(*mut c_void, *mut VideoFrameV2, *mut c_void, *mut c_void, u32) -> c_int;
type FnRecvFreeVideoV2 = unsafe extern "C" fn(*mut c_void, *const VideoFrameV2);
type FnRecvGetNoConnections = unsafe extern "C" fn(*mut c_void) -> c_int;

/// The entry points this crate needs, and no more.
pub struct Api {
    // Kept so the library outlives every symbol resolved out of it.
    _lib: libloading::Library,
    pub path: PathBuf,
    pub version: String,
    send_create: FnSendCreate,
    send_destroy: FnSendDestroy,
    send_video_v2: FnSendVideoV2,
    send_get_no_connections: Option<FnSendGetNoConnections>,
    find_create_v2: FnFindCreateV2,
    find_destroy: FnFindDestroy,
    find_get_current_sources: FnFindGetCurrentSources,
    find_wait_for_sources: FnFindWaitForSources,
    recv_create_v3: FnRecvCreateV3,
    recv_destroy: FnRecvDestroy,
    recv_capture_v2: FnRecvCaptureV2,
    recv_free_video_v2: FnRecvFreeVideoV2,
    recv_get_no_connections: Option<FnRecvGetNoConnections>,
}

// SAFETY: every entry point bound here is documented thread-safe by the SDK, and
// the handles below are only ever touched through `&mut self`.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

/// Loads the runtime once per process. Repeated calls hand back the same [`Api`];
/// a failure is cached too, so a missing runtime costs one search and not one per
/// source.
pub fn load() -> Result<Arc<Api>> {
    static ONCE: std::sync::OnceLock<Result<Arc<Api>, String>> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| load_uncached().map_err(|e| e.to_string()))
        .clone()
        .map_err(|e| anyhow!(e))
}

fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for var in [REDIST_FOLDER_VAR, REDIST_FOLDER_VAR_V5] {
        if let Some(dir) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            for name in LIBRARY_NAMES {
                out.push(PathBuf::from(&dir).join(name));
            }
        }
    }
    // Beside the executable: this is what lets a signed installer ship the
    // library in the app's own folder with no code change.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in LIBRARY_NAMES {
                out.push(dir.join(name));
            }
            // Inside a macOS .app the runtime lives in Contents/Frameworks,
            // because `codesign` will not accept a dylib anywhere else — and that
            // is not always beside the executable, so walk up for the enclosing
            // `Contents` rather than assuming a fixed depth.
            #[cfg(target_os = "macos")]
            if let Some(contents) = dir
                .ancestors()
                .find(|a| a.file_name().is_some_and(|n| n == "Contents"))
            {
                for name in LIBRARY_NAMES {
                    out.push(contents.join("Frameworks").join(name));
                }
            }
        }
    }
    // Bare name: the platform's own loader search path.
    for name in LIBRARY_NAMES {
        out.push(PathBuf::from(name));
    }
    for dir in EXTRA_DIRS {
        for name in LIBRARY_NAMES {
            out.push(PathBuf::from(dir).join(name));
        }
    }
    out
}

fn load_uncached() -> Result<Arc<Api>> {
    let mut lib = None;
    for path in &candidates() {
        // SAFETY: loading a shared library runs its initialisers. This is the
        // vendor's own signed runtime, found at a path whose shape we control.
        if let Ok(handle) = unsafe { libloading::Library::new(path) } {
            lib = Some((handle, path.clone()));
            break;
        }
    }
    let (lib, path) = lib.ok_or_else(|| {
        anyhow!(
            "NDI runtime not found (tried {}). Install it from {REDIST_URL}, \
             or set {REDIST_FOLDER_VAR} to the directory containing it.",
            LIBRARY_NAMES.join(", "),
        )
    })?;

    // SAFETY: each name is bound to the signature declared in the SDK headers;
    // see this module's header comment. `libloading::Library::get` is the typed
    // lookup, so no transmute is involved. A missing required symbol is an error,
    // not a panic, so an NDI 4 runtime degrades cleanly.
    unsafe {
        macro_rules! required {
            ($ty:ty, $name:literal) => {
                *lib.get::<$ty>(concat!($name, "\0").as_bytes()).map_err(|_| {
                    anyhow!(
                        "NDI runtime at {} is missing {} — it is too old (NDI 5 or newer is needed)",
                        path.display(),
                        $name,
                    )
                })?
            };
        }

        let initialize = required!(FnInitialize, "NDIlib_initialize");
        if !initialize() {
            return Err(anyhow!(
                "the NDI runtime at {} refused to initialise — this CPU is not supported by it",
                path.display()
            ));
        }

        let version = lib
            .get::<FnVersion>(b"NDIlib_version\0")
            .ok()
            .map(|f| CStr::from_ptr(f()).to_string_lossy().into_owned())
            .unwrap_or_default();

        let api = Api {
            send_create: required!(FnSendCreate, "NDIlib_send_create"),
            send_destroy: required!(FnSendDestroy, "NDIlib_send_destroy"),
            send_video_v2: required!(FnSendVideoV2, "NDIlib_send_send_video_v2"),
            send_get_no_connections: lib
                .get::<FnSendGetNoConnections>(b"NDIlib_send_get_no_connections\0")
                .ok()
                .map(|f| *f),
            find_create_v2: required!(FnFindCreateV2, "NDIlib_find_create_v2"),
            find_destroy: required!(FnFindDestroy, "NDIlib_find_destroy"),
            find_get_current_sources: required!(
                FnFindGetCurrentSources,
                "NDIlib_find_get_current_sources"
            ),
            find_wait_for_sources: required!(FnFindWaitForSources, "NDIlib_find_wait_for_sources"),
            recv_create_v3: required!(FnRecvCreateV3, "NDIlib_recv_create_v3"),
            recv_destroy: required!(FnRecvDestroy, "NDIlib_recv_destroy"),
            recv_capture_v2: required!(FnRecvCaptureV2, "NDIlib_recv_capture_v2"),
            recv_free_video_v2: required!(FnRecvFreeVideoV2, "NDIlib_recv_free_video_v2"),
            recv_get_no_connections: lib
                .get::<FnRecvGetNoConnections>(b"NDIlib_recv_get_no_connections\0")
                .ok()
                .map(|f| *f),
            path,
            version,
            _lib: lib,
        };
        Ok(Arc::new(api))
    }
}

// `NDIlib_destroy` is deliberately never called: at process teardown it races the
// SDK's own worker threads, and the process is exiting anyway.

/// A discovered source.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceName {
    /// The full NDI name, e.g. `STUDIO-PC (Arena - Screen 1)`.
    pub name: String,
    pub url: Option<String>,
}

/// A discovery handle.
pub struct Finder {
    api: Arc<Api>,
    instance: *mut c_void,
}

// SAFETY: every method takes `&mut self`, so no two calls overlap.
unsafe impl Send for Finder {}

impl Finder {
    pub fn new(api: Arc<Api>, show_local_sources: bool) -> Result<Self> {
        let settings = FindCreate {
            show_local_sources,
            p_groups: std::ptr::null(),
            p_extra_ips: std::ptr::null(),
        };
        // SAFETY: `settings` outlives the call, which copies what it needs.
        let instance = unsafe { (api.find_create_v2)(&settings) };
        if instance.is_null() {
            return Err(anyhow!(
                "the NDI runtime at {} would not create a finder",
                api.path.display()
            ));
        }
        Ok(Self { api, instance })
    }

    /// `true` if the source list changed before the timeout expired.
    pub fn wait_for_sources(&mut self, timeout_ms: u32) -> bool {
        // SAFETY: `instance` is non-null for this type's whole lifetime.
        unsafe { (self.api.find_wait_for_sources)(self.instance, timeout_ms) }
    }

    pub fn sources(&mut self) -> Vec<SourceName> {
        let mut count: u32 = 0;
        // SAFETY: the returned array is owned by the finder and valid until the
        // next call on it, so every string is copied out before returning.
        unsafe {
            let ptr = (self.api.find_get_current_sources)(self.instance, &mut count);
            if ptr.is_null() || count == 0 {
                return Vec::new();
            }
            std::slice::from_raw_parts(ptr, count as usize)
                .iter()
                .map(|raw| SourceName {
                    name: if raw.p_ndi_name.is_null() {
                        String::new()
                    } else {
                        CStr::from_ptr(raw.p_ndi_name)
                            .to_string_lossy()
                            .into_owned()
                    },
                    url: if raw.p_url_address.is_null() {
                        None
                    } else {
                        Some(
                            CStr::from_ptr(raw.p_url_address)
                                .to_string_lossy()
                                .into_owned(),
                        )
                    },
                })
                .collect()
        }
    }
}

impl Drop for Finder {
    fn drop(&mut self) {
        // SAFETY: called once, on a handle this type exclusively owns.
        unsafe { (self.api.find_destroy)(self.instance) }
    }
}

/// A receiver connected to one source.
pub struct Receiver {
    api: Arc<Api>,
    instance: *mut c_void,
    // Kept alive for as long as the receiver: the SDK copies the name at create
    // time, but keeping it costs nothing and removes a class of doubt.
    _name: CString,
}

// SAFETY: the SDK documents a receive instance as usable from one thread at a
// time; every method takes `&mut self`.
unsafe impl Send for Receiver {}

/// What [`Receiver::capture`] found.
pub enum Captured<'a> {
    /// Borrowed from the SDK and freed when the guard drops.
    Video(VideoGuard<'a>),
    /// A frame arrived that is not video, or the source's format changed.
    Other,
    /// Nothing arrived before the timeout. Normal and frequent.
    Nothing,
}

/// A borrowed video frame. Freeing it back to the SDK is not optional — the
/// runtime's frame pool is finite, and leaking a few frames stalls the receiver —
/// so the free happens in `Drop` rather than at a call site that could be missed.
pub struct VideoGuard<'a> {
    receiver: &'a Receiver,
    frame: VideoFrameV2,
}

impl VideoGuard<'_> {
    pub fn width(&self) -> u32 {
        self.frame.xres.max(0) as u32
    }

    pub fn height(&self) -> u32 {
        self.frame.yres.max(0) as u32
    }

    pub fn four_cc(&self) -> u32 {
        self.frame.four_cc
    }

    pub fn stride(&self) -> usize {
        self.frame.line_stride_in_bytes.max(0) as usize
    }

    pub fn frame_rate(&self) -> (i32, i32) {
        (self.frame.frame_rate_n, self.frame.frame_rate_d)
    }

    /// The pixel data, or `None` if the frame carries no readable buffer.
    ///
    /// Only valid until this guard drops.
    pub fn data(&self) -> Option<&[u8]> {
        if self.frame.p_data.is_null() || self.height() == 0 || self.stride() == 0 {
            return None;
        }
        // SAFETY: the SDK guarantees `yres * line_stride_in_bytes` readable bytes
        // at `p_data` until the frame is freed, which Drop does and which cannot
        // happen while this borrow is live.
        Some(unsafe {
            std::slice::from_raw_parts(self.frame.p_data, self.height() as usize * self.stride())
        })
    }
}

impl Drop for VideoGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: frees exactly the frame the SDK filled in, once.
        unsafe { (self.receiver.api.recv_free_video_v2)(self.receiver.instance, &self.frame) }
    }
}

impl Receiver {
    /// Connect to `source`. `recv_name` is what this receiver calls itself on the
    /// network, which is what a sender's connection list shows.
    pub fn new(api: Arc<Api>, source: &str, recv_name: &str, low_bandwidth: bool) -> Result<Self> {
        let c_source = CString::new(source)?;
        let c_recv = CString::new(recv_name)?;

        let settings = RecvCreateV3 {
            source_to_connect_to: SourceRaw {
                p_ndi_name: c_source.as_ptr(),
                p_url_address: std::ptr::null(),
            },
            color_format: RECV_COLOR_RGBX_RGBA,
            bandwidth: if low_bandwidth {
                RECV_BANDWIDTH_LOWEST
            } else {
                RECV_BANDWIDTH_HIGHEST
            },
            // Interlaced fields would arrive as half-height frames and quietly
            // halve the vertical resolution of a wall. Refuse them instead.
            allow_video_fields: false,
            p_ndi_recv_name: c_recv.as_ptr(),
        };

        // SAFETY: `settings` and both strings outlive the call, which copies what
        // it needs.
        let instance = unsafe { (api.recv_create_v3)(&settings) };
        if instance.is_null() {
            return Err(anyhow!(
                "the NDI runtime at {} would not create a receiver for {source:?}",
                api.path.display()
            ));
        }
        Ok(Self {
            api,
            instance,
            _name: c_recv,
        })
    }

    /// Wait up to `timeout_ms` for a frame.
    ///
    /// Audio and metadata are passed as null, which tells the SDK to discard them
    /// rather than queue them — UnMapper is a video tool, and a queue nothing
    /// drains is a leak.
    pub fn capture(&mut self, timeout_ms: u32) -> Captured<'_> {
        let mut frame = VideoFrameV2::default();
        // SAFETY: `instance` is non-null for this type's lifetime; `frame` is a
        // valid out-parameter; nulls are the documented way to decline audio and
        // metadata.
        let kind = unsafe {
            (self.api.recv_capture_v2)(
                self.instance,
                &mut frame,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                timeout_ms,
            )
        };

        match kind {
            FRAME_TYPE_VIDEO => Captured::Video(VideoGuard {
                receiver: self,
                frame,
            }),
            FRAME_TYPE_NONE => Captured::Nothing,
            _ => Captured::Other,
        }
    }

    /// Whether this receiver is currently connected to its sender.
    pub fn is_connected(&self) -> bool {
        match self.api.recv_get_no_connections {
            // SAFETY: as `capture`.
            Some(f) => (unsafe { f(self.instance) }) > 0,
            // A runtime that does not export the call cannot be asked; assume yes
            // rather than reporting a disconnection that may not exist.
            None => true,
        }
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        // SAFETY: called once, on a handle this type exclusively owns.
        unsafe { (self.api.recv_destroy)(self.instance) }
    }
}

/// An NDI sender.
pub struct Sender {
    api: Arc<Api>,
    instance: *mut c_void,
}

// SAFETY: as `Receiver`.
unsafe impl Send for Sender {}

impl Sender {
    pub fn new(api: Arc<Api>, name: &str) -> Result<Self> {
        let c_name = CString::new(name)?;
        let settings = SendCreate {
            p_ndi_name: c_name.as_ptr(),
            p_groups: std::ptr::null(),
            // The render loop paces frames; letting the SDK also clock them would
            // fight it.
            clock_video: false,
            clock_audio: false,
        };
        // SAFETY: `settings` and the string it points at both outlive the call.
        let instance = unsafe { (api.send_create)(&settings) };
        if instance.is_null() {
            return Err(anyhow!(
                "the NDI runtime at {} would not create a sender named {name:?}",
                api.path.display()
            ));
        }
        Ok(Self { api, instance })
    }

    /// `frame.p_data` must point at `yres * line_stride_in_bytes` readable bytes;
    /// this call copies before returning.
    pub fn send_video(&mut self, frame: &VideoFrameV2) {
        // SAFETY: `instance` is non-null and the frame is valid for the call.
        unsafe { (self.api.send_video_v2)(self.instance, frame) }
    }

    pub fn connections(&mut self, timeout_ms: u32) -> Option<i32> {
        let f = self.api.send_get_no_connections?;
        // SAFETY: as `send_video`.
        Some(unsafe { f(self.instance, timeout_ms) })
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        // SAFETY: called once, on a handle this type exclusively owns.
        unsafe { (self.api.send_destroy)(self.instance) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These are the guard against a layout typo becoming silent memory
    /// corruption. They check against the C declarations in
    /// `Processing.NDI.structs.h` / `.Recv.h`, computed by hand from the field
    /// types and the platform's alignment rules.
    #[test]
    fn struct_layouts_match_the_sdk_headers() {
        let ptr = std::mem::size_of::<*const c_char>();
        assert_eq!(ptr, 8, "these expectations assume a 64-bit target");

        // NDIlib_source_t: two pointers.
        assert_eq!(std::mem::size_of::<SourceRaw>(), 2 * ptr);
        assert_eq!(std::mem::align_of::<SourceRaw>(), ptr);

        // NDIlib_find_create_t: bool, then two pointers — the bool pads out to
        // the pointer alignment.
        assert_eq!(std::mem::size_of::<FindCreate>(), 3 * ptr);
        assert_eq!(std::mem::align_of::<FindCreate>(), ptr);

        // NDIlib_send_create_t: two pointers, two bools, padded to alignment.
        assert_eq!(std::mem::size_of::<SendCreate>(), 3 * ptr);

        // NDIlib_video_frame_v2_t: 7 x int/float (28) + 4 pad + int64 (8)
        // + ptr (8) + int (4) + 4 pad + ptr (8) + int64 (8) = 72.
        assert_eq!(std::mem::size_of::<VideoFrameV2>(), 72);
        assert_eq!(std::mem::align_of::<VideoFrameV2>(), 8);

        // NDIlib_recv_create_v3_t: source (16) + 2 int (8) + bool (1)
        // + 7 pad + ptr (8) = 40.
        assert_eq!(std::mem::size_of::<RecvCreateV3>(), 40);
        assert_eq!(std::mem::align_of::<RecvCreateV3>(), ptr);
    }

    #[test]
    fn field_offsets_are_in_declaration_order() {
        // A reordered field would keep the same total size and still be wrong, so
        // size alone is not enough.
        let f = VideoFrameV2::default();
        let base = &f as *const _ as usize;
        assert_eq!(&f.xres as *const _ as usize - base, 0);
        assert_eq!(&f.yres as *const _ as usize - base, 4);
        assert_eq!(&f.four_cc as *const _ as usize - base, 8);
        assert_eq!(&f.timecode as *const _ as usize - base, 32);
        assert_eq!(&f.p_data as *const _ as usize - base, 40);
        assert_eq!(&f.line_stride_in_bytes as *const _ as usize - base, 48);
        assert_eq!(&f.timestamp as *const _ as usize - base, 64);

        let r = RecvCreateV3 {
            source_to_connect_to: SourceRaw::default(),
            color_format: 0,
            bandwidth: 0,
            allow_video_fields: false,
            p_ndi_recv_name: std::ptr::null(),
        };
        let base = &r as *const _ as usize;
        assert_eq!(&r.source_to_connect_to as *const _ as usize - base, 0);
        assert_eq!(&r.color_format as *const _ as usize - base, 16);
        assert_eq!(&r.bandwidth as *const _ as usize - base, 20);
        assert_eq!(&r.allow_video_fields as *const _ as usize - base, 24);
        assert_eq!(&r.p_ndi_recv_name as *const _ as usize - base, 32);
    }

    #[test]
    fn fourcc_constants_are_little_endian_packed() {
        // NDI writes these as four chars in memory order, so on a little-endian
        // target the u32 reads back reversed. Getting this backwards would make
        // every frame appear to be in an unknown format.
        assert_eq!(FOURCC_RGBA, 0x41424752);
        assert_eq!(FOURCC_UYVY, 0x59565955);
    }
}
