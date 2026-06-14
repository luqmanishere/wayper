//! Declaration of scene

use std::path::PathBuf;

use wayper_lib::config::FitMode;

#[derive(Debug, Clone)]
pub struct Scene {
    pub background: [f32; 4],
    pub nodes: Vec<SceneNode>,
}

#[derive(Debug, Clone)]
pub enum SceneNode {
    Image(ImageNode),
}

#[derive(Debug, Clone)]
pub struct ImageNode {
    pub image_path: PathBuf,
    pub opacity: f32,
    pub rect: Rect,
    pub fit: FitMode,
}

impl ImageNode {
    /// Create a new fullscreen image
    pub fn fullscreen(image_path: PathBuf, output_size: (u32, u32), fit: FitMode) -> Self {
        Self {
            image_path,
            opacity: 1.0,
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: output_size.0 as f32,
                height: output_size.1 as f32,
            },
            fit,
        }
    }

    /// opacity setter, range is 0.0 -> 1.0 clamped
    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity.clamp(0.0, 1.0);
        self
    }

    /// rect setter, represents position on the screen
    pub fn with_rect(mut self, rect: Rect) -> Self {
        self.rect = rect;
        self
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn as_array(&self) -> [f32; 4] {
        [self.x, self.y, self.width, self.height]
    }
}
