pub mod compositor;

/// Unique identifier for a source within a scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceId(pub u32);

/// How a source is scaled when its native resolution differs from the scene canvas.
#[derive(Debug, Clone, Copy)]
pub enum ScaleMode {
    /// Stretch to fill the item bounds, ignoring aspect ratio.
    Stretch,
    /// Fit within the item bounds while preserving aspect ratio (letterbox).
    Fit,
    /// Fill the item bounds while preserving aspect ratio (crop).
    Fill,
    /// No scaling; render at native resolution (may crop or leave gaps).
    None,
}

/// Position and size of a source within the scene canvas, in pixels.
#[derive(Debug, Clone, Copy)]
pub struct Transform {
    /// Left edge of the item on the scene canvas.
    pub x: f32,
    /// Top edge of the item on the scene canvas.
    pub y: f32,
    /// Width of the item on the scene canvas.
    pub width: f32,
    /// Height of the item on the scene canvas.
    pub height: f32,
    /// Rotation in degrees (clockwise). Phase 1 ignores this.
    pub rotation: f32,
    pub scale_mode: ScaleMode,
}

impl Transform {
    /// A transform that fills the entire canvas (for a single full-screen source).
    pub fn fullscreen(canvas_width: u32, canvas_height: u32) -> Self {
        Transform {
            x: 0.0,
            y: 0.0,
            width: canvas_width as f32,
            height: canvas_height as f32,
            rotation: 0.0,
            scale_mode: ScaleMode::Fit,
        }
    }
}

/// One source placed in a scene, with its position and visibility.
#[derive(Debug, Clone)]
pub struct SceneItem {
    pub source_id: SourceId,
    pub transform: Transform,
    pub visible: bool,
    /// Lower z-order is drawn first (further back).
    pub z_order: i32,
}

/// A collection of sources composited together to form one video output.
#[derive(Debug)]
pub struct Scene {
    pub name: String,
    pub canvas_width: u32,
    pub canvas_height: u32,
    items: Vec<SceneItem>,
}

impl Scene {
    pub fn new(name: impl Into<String>, canvas_width: u32, canvas_height: u32) -> Self {
        Scene {
            name: name.into(),
            canvas_width,
            canvas_height,
            items: Vec::new(),
        }
    }

    pub fn add_item(&mut self, item: SceneItem) {
        self.items.push(item);
        // Keep items sorted by z-order so the compositor can iterate in order.
        self.items.sort_by_key(|i| i.z_order);
    }

    pub fn items(&self) -> &[SceneItem] {
        &self.items
    }

    pub fn items_mut(&mut self) -> &mut Vec<SceneItem> {
        &mut self.items
    }
}
