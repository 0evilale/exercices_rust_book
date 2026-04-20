use std::collections::HashMap;

use tracing::debug;

use crate::types::{PixelFormat, Timestamp, VideoFrame};

use super::{ScaleMode, Scene, SourceId};

/// Blends the most recent frame from each visible source into a single canvas frame.
///
/// Algorithm:
/// 1. Allocate a zeroed (black) BGRA32 canvas of `scene.canvas_width × scene.canvas_height`.
/// 2. Iterate scene items in ascending z-order.
/// 3. For each visible item that has a frame: blit/scale the source frame into the
///    destination rectangle using nearest-neighbour scaling.
/// 4. Return the composited `VideoFrame`.
pub struct Compositor {
    scene: Scene,
    /// Latest frame received from each source, keyed by SourceId.
    frame_cache: HashMap<SourceId, VideoFrame>,
}

impl Compositor {
    pub fn new(scene: Scene) -> Self {
        Compositor {
            scene,
            frame_cache: HashMap::new(),
        }
    }

    /// Update the cached frame for a source.
    pub fn update_source(&mut self, id: SourceId, frame: VideoFrame) {
        self.frame_cache.insert(id, frame);
    }

    /// Composite all visible sources and return the merged frame.
    pub fn composite(&self, pts: Timestamp) -> VideoFrame {
        let cw = self.scene.canvas_width as usize;
        let ch = self.scene.canvas_height as usize;
        let mut canvas = vec![0u8; cw * ch * 4]; // BGRA32, black

        for item in self.scene.items() {
            if !item.visible {
                continue;
            }
            let Some(src_frame) = self.frame_cache.get(&item.source_id) else {
                continue;
            };
            blit_frame(
                src_frame,
                &item.transform,
                &mut canvas,
                self.scene.canvas_width,
                self.scene.canvas_height,
            );
        }

        debug!(
            "composited {} items → {}×{} canvas",
            self.scene.items().len(),
            cw,
            ch
        );

        VideoFrame::new(
            self.scene.canvas_width,
            self.scene.canvas_height,
            PixelFormat::BGRA32,
            canvas,
            pts,
        )
    }

    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }
}

// ── Blit helper ──────────────────────────────────────────────────────────────

/// Blit `src_frame` into `canvas` at the rectangle described by `transform`,
/// using nearest-neighbour scaling.
fn blit_frame(
    src: &VideoFrame,
    transform: &super::Transform,
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
) {
    // Compute the destination rectangle in integer canvas coordinates.
    let dst_x = transform.x as i32;
    let dst_y = transform.y as i32;
    let (dst_w, dst_h) = compute_dst_size(src, transform);

    let src_w = src.width as usize;
    let src_h = src.height as usize;
    let cw = canvas_width as usize;
    let ch = canvas_height as usize;

    if dst_w == 0 || dst_h == 0 {
        return;
    }

    let src_data = src.data.as_ref();

    // Determine the bytes-per-pixel for the source format.
    let src_bpp = match src.format {
        PixelFormat::BGRA32 | PixelFormat::RGBA32 => 4usize,
        PixelFormat::I420 => return, // not handled in blit; encoder converts this
    };

    for py in 0..dst_h {
        let cy = dst_y + py as i32;
        if cy < 0 || cy >= ch as i32 {
            continue;
        }

        // Map destination row → source row (nearest-neighbour)
        let sy = (py * src_h) / dst_h;
        let sy = sy.min(src_h - 1);

        for px in 0..dst_w {
            let cx = dst_x + px as i32;
            if cx < 0 || cx >= cw as i32 {
                continue;
            }

            // Map destination col → source col (nearest-neighbour)
            let sx = (px * src_w) / dst_w;
            let sx = sx.min(src_w - 1);

            let src_off = (sy * src_w + sx) * src_bpp;
            let dst_off = (cy as usize * cw + cx as usize) * 4;

            // Read source pixel → write as BGRA to canvas.
            let (b, g, r, a) = match src.format {
                PixelFormat::BGRA32 => (
                    src_data[src_off],
                    src_data[src_off + 1],
                    src_data[src_off + 2],
                    src_data[src_off + 3],
                ),
                PixelFormat::RGBA32 => (
                    src_data[src_off + 2], // swap R and B
                    src_data[src_off + 1],
                    src_data[src_off],
                    src_data[src_off + 3],
                ),
                PixelFormat::I420 => unreachable!(),
            };

            if a == 255 {
                // Fast path: fully opaque → simple copy
                canvas[dst_off] = b;
                canvas[dst_off + 1] = g;
                canvas[dst_off + 2] = r;
                canvas[dst_off + 3] = 255;
            } else if a > 0 {
                // Alpha blend: dst = src·alpha + dst·(1-alpha)
                let alpha = a as u16;
                let inv = 255 - alpha;
                canvas[dst_off] = ((b as u16 * alpha + canvas[dst_off] as u16 * inv) / 255) as u8;
                canvas[dst_off + 1] =
                    ((g as u16 * alpha + canvas[dst_off + 1] as u16 * inv) / 255) as u8;
                canvas[dst_off + 2] =
                    ((r as u16 * alpha + canvas[dst_off + 2] as u16 * inv) / 255) as u8;
                canvas[dst_off + 3] = 255;
            }
            // a == 0 → fully transparent, skip
        }
    }
}

/// Compute the destination (width, height) in pixels for a source frame placed
/// with the given transform, respecting the ScaleMode.
fn compute_dst_size(src: &VideoFrame, transform: &super::Transform) -> (usize, usize) {
    let box_w = transform.width as usize;
    let box_h = transform.height as usize;

    match transform.scale_mode {
        ScaleMode::Stretch | ScaleMode::None => (box_w, box_h),
        ScaleMode::Fit => {
            let src_aspect = src.width as f64 / src.height as f64;
            let box_aspect = box_w as f64 / box_h as f64;
            if src_aspect > box_aspect {
                // Source is wider: constrain by width
                let h = (box_w as f64 / src_aspect) as usize;
                (box_w, h)
            } else {
                // Source is taller: constrain by height
                let w = (box_h as f64 * src_aspect) as usize;
                (w, box_h)
            }
        }
        ScaleMode::Fill => {
            let src_aspect = src.width as f64 / src.height as f64;
            let box_aspect = box_w as f64 / box_h as f64;
            if src_aspect > box_aspect {
                // Source is wider: constrain by height (crop sides)
                let w = (box_h as f64 * src_aspect) as usize;
                (w, box_h)
            } else {
                // Source is taller: constrain by width (crop top/bottom)
                let h = (box_w as f64 / src_aspect) as usize;
                (box_w, h)
            }
        }
    }
}
