//! Sources that do not come off the network: still images and the test pattern.
//!
//! Both exist so a rig can be laid out and checked with nothing plugged in —
//! no Resolume, no network, often no venue. They upload once and never change,
//! unlike an NDI feed, so the caller tracks what it has already done.

use std::path::Path;

use anyhow::{Context, Result};
use unmapper_core::{Show, Size, SourceKind};

use crate::{FrameUpload, Gpu, SourceTextures};

/// What was uploaded, for a caller that wants to report it.
pub struct Uploaded {
    pub source_id: String,
    pub size: Size,
    pub what: &'static str,
}

/// Upload every still and test-pattern source that is not already present.
///
/// Returns what it did, and any per-source error — one bad path should not stop
/// the other sources from loading.
pub fn sync_offline_sources(
    gpu: &Gpu,
    textures: &mut SourceTextures,
    show: &Show,
) -> (Vec<Uploaded>, Vec<String>) {
    let mut done = Vec::new();
    let mut errors = Vec::new();

    for source in show.sources.iter().filter(|s| s.enabled) {
        // Already uploaded. These never change, so there is nothing to refresh —
        // a still is a file and the pattern is a pure function of its size.
        if textures.get(&source.id).is_some() {
            continue;
        }

        match &source.kind {
            SourceKind::TestPattern => {
                // Sized to what the slice map says the screen is, so slices land
                // where they would on the real feed and the grid lines up.
                let size = source.expected.unwrap_or(Size::new(1920, 1080));
                let data = crate::test_pattern(size);
                upload(gpu, textures, &source.id, size, &data);
                done.push(Uploaded {
                    source_id: source.id.clone(),
                    size,
                    what: "test pattern",
                });
            }
            SourceKind::Still { path } => match load_image(path) {
                Ok((size, data)) => {
                    upload(gpu, textures, &source.id, size, &data);
                    done.push(Uploaded {
                        source_id: source.id.clone(),
                        size,
                        what: "still",
                    });
                }
                Err(e) => errors.push(format!("source {:?}: {e:#}", source.name)),
            },
            SourceKind::Ndi { .. } => {}
        }
    }

    (done, errors)
}

fn upload(gpu: &Gpu, textures: &mut SourceTextures, id: &str, size: Size, data: &[u8]) {
    textures.upload(
        gpu,
        id,
        FrameUpload {
            width: size.width,
            height: size.height,
            stride: (size.width * 4) as usize,
            bgra: false,
            data,
            sequence: 0,
        },
    );
}

pub fn load_image(path: &Path) -> Result<(Size, Vec<u8>)> {
    let img = image::ImageReader::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .decode()
        .with_context(|| format!("decoding {}", path.display()))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    Ok((Size::new(w, h), img.into_raw()))
}
