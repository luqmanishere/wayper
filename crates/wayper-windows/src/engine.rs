use std::{sync::Arc, time::Instant};

use color_eyre::Result;
use wayper_windows::config::{Config, ResolvedContent};
use wgpu::naga::FastHashMap;
use winit::{
    dpi::PhysicalSize,
    window::{Window, WindowId},
};

use crate::{
    player::{FramePlan, Player},
    renderer::Renderer,
};

pub struct Engine {
    renderer: Renderer,
    outputs: FastHashMap<String, OutputState>,
    window_id_iden_map: FastHashMap<WindowId, String>,
    config: Config,
}

pub struct OutputState {
    pub window: Arc<Window>,
    pub size: PhysicalSize<u32>,
    pub player: Player,
    pub dirty: bool,
}

pub struct SchedulePlan {
    pub redraw_outputs: Vec<String>,
    pub next_wakeup: Option<Instant>,
}

impl Engine {
    /// Create a new instance of Engine. Internally, it also creates a new renderer.
    pub async fn new(config: Config) -> Result<Self> {
        Ok(Self {
            renderer: Renderer::new().await?,
            outputs: Default::default(),
            window_id_iden_map: Default::default(),
            config,
        })
    }

    /// Whether the output is registered in the engine
    pub fn has_output(&self, output_iden: &str) -> bool {
        self.outputs.contains_key(output_iden)
    }

    /// Get the window for the output
    pub fn get_window(&self, output_iden: &str) -> Option<&Arc<Window>> {
        self.outputs.get(output_iden).map(|output| &output.window)
    }

    /// Get the output_id for the provided window id
    pub fn output_id_for_window(&self, window_id: &WindowId) -> Option<&String> {
        self.window_id_iden_map.get(window_id)
    }

    /// Register an output with the engine
    pub fn add_output(
        &mut self,
        output_iden: String,
        window: Arc<Window>,
        size: PhysicalSize<u32>,
    ) -> Result<()> {
        let resolved = self.config.resolve_output_content(&output_iden)?;
        self.renderer.create_surface(
            output_iden.clone(),
            (size.width, size.height),
            window.clone(),
        )?;

        let player = match resolved {
            ResolvedContent::Image(image) => {
                Player::image(image.path, image.fit.into(), image.alignment.into())
            }
            ResolvedContent::Video(_) => {
                color_eyre::eyre::bail!("video content is not implemented yet in wayper-windows")
            }
            ResolvedContent::Scene(_) => {
                color_eyre::eyre::bail!("scene content is not implemented yet in wayper-windows")
            }
        };

        self.window_id_iden_map
            .insert(window.id(), output_iden.clone());
        self.outputs.insert(
            output_iden,
            OutputState {
                window,
                size,
                player,
                dirty: true,
            },
        );
        Ok(())
    }

    /// Resize an output to the provided size. If the output has no attached window/surface, it will
    /// error.
    pub fn resize_output(&mut self, output_iden: &str, new_size: PhysicalSize<u32>) -> Result<()> {
        if let Some(output) = self.outputs.get_mut(output_iden) {
            output.size = new_size;
            output.dirty = true;
        }
        self.renderer
            .resize_surface(output_iden, (new_size.width, new_size.height))
    }

    /// Render for an output
    pub fn render_output(&mut self, output_iden: &str, now: Instant) -> Result<()> {
        let output = self
            .outputs
            .get_mut(output_iden)
            .expect("output must exist before rendering");

        output.player.prepare(&mut self.renderer)?;
        let scene = output.player.render_scene(now)?;
        self.renderer.render(output_iden, &scene)?;
        output.player.mark_rendered();
        output.dirty = false;

        Ok(())
    }

    pub fn schedule(&mut self, now: Instant) -> SchedulePlan {
        let mut redraw_outputs = Vec::new();
        let mut next_wakeup: Option<Instant> = None;

        for (output_iden, output) in &mut self.outputs {
            let FramePlan {
                render_now,
                next_wakeup: player_wakeup,
            } = output.player.update(now);

            if output.dirty || render_now {
                redraw_outputs.push(output_iden.clone());
            }

            if let Some(wakeup) = player_wakeup {
                next_wakeup = Some(match next_wakeup {
                    Some(current) => current.min(wakeup),
                    None => wakeup,
                });
            }
        }

        SchedulePlan {
            redraw_outputs,
            next_wakeup,
        }
    }
}
