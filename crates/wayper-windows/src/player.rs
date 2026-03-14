use std::{path::PathBuf, time::Instant};

use color_eyre::{Result, eyre::OptionExt};
use wayper_windows::config::{
    AlignmentConfig, FitModeConfig, HorizontalAlignmentConfig, VerticalAlignmentConfig,
};

use crate::renderer::{FitAlignment, FitMode, RenderScene, Renderer, TextureHandle};

impl From<FitModeConfig> for FitMode {
    fn from(value: FitModeConfig) -> Self {
        match value {
            FitModeConfig::Contain => FitMode::Contain,
            FitModeConfig::Cover => FitMode::Cover,
            FitModeConfig::Stretch => FitMode::Stretch,
        }
    }
}

impl From<AlignmentConfig> for FitAlignment {
    fn from(value: AlignmentConfig) -> Self {
        Self {
            x: match value.horizontal {
                HorizontalAlignmentConfig::Left => 0.0,
                HorizontalAlignmentConfig::Center => 0.5,
                HorizontalAlignmentConfig::Right => 1.0,
            },
            y: match value.vertical {
                VerticalAlignmentConfig::Top => 0.0,
                VerticalAlignmentConfig::Center => 0.5,
                VerticalAlignmentConfig::Bottom => 1.0,
            },
        }
    }
}

pub struct FramePlan {
    pub render_now: bool,
    pub next_wakeup: Option<Instant>,
}

pub enum Player {
    Image(ImagePlayer),
}

impl Player {
    pub fn image(image_path: PathBuf, fit: FitMode, alignment: FitAlignment) -> Self {
        Self::Image(ImagePlayer::new(image_path, fit, alignment))
    }

    pub fn update(&mut self, now: Instant) -> FramePlan {
        match self {
            Player::Image(player) => player.update(now),
        }
    }

    pub fn prepare(&mut self, renderer: &mut Renderer) -> Result<()> {
        match self {
            Player::Image(player) => player.prepare(renderer),
        }
    }

    pub fn render_scene(&self, now: Instant) -> Result<RenderScene> {
        match self {
            Player::Image(player) => player.render_scene(now),
        }
    }

    pub fn mark_rendered(&mut self) {
        match self {
            Player::Image(player) => player.mark_rendered(),
        }
    }
}

pub struct ImagePlayer {
    image_path: PathBuf,
    fit: FitMode,
    alignment: FitAlignment,
    texture: Option<TextureHandle>,
    image_size: Option<(u32, u32)>,
    dirty: bool,
}

impl ImagePlayer {
    pub fn new(image_path: PathBuf, fit: FitMode, alignment: FitAlignment) -> Self {
        Self {
            image_path,
            fit,
            alignment,
            texture: None,
            image_size: None,
            dirty: true,
        }
    }

    pub fn update(&mut self, _now: Instant) -> FramePlan {
        FramePlan {
            render_now: self.dirty,
            next_wakeup: None,
        }
    }

    pub fn prepare(&mut self, renderer: &mut Renderer) -> Result<()> {
        if self.texture.is_some() {
            return Ok(());
        }

        let image = image::open(&self.image_path)?;
        let rgba = image.to_rgba8();
        let dimensions = rgba.dimensions();
        let texture = renderer.upload_rgba_image(&rgba, dimensions)?;
        self.texture = Some(texture);
        self.image_size = Some(dimensions);
        self.dirty = true;

        Ok(())
    }

    pub fn render_scene(&self, _now: Instant) -> Result<RenderScene> {
        let texture = self
            .texture
            .ok_or_eyre("image player must upload its texture before rendering")?;
        let image_size = self
            .image_size
            .ok_or_eyre("image player must know image dimensions before rendering")?;

        Ok(RenderScene::Image {
            texture,
            image_size,
            fit: self.fit,
            alignment: self.alignment,
        })
    }

    pub fn mark_rendered(&mut self) {
        self.dirty = false;
    }
}
