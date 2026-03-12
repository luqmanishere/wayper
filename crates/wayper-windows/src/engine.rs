use std::{sync::Arc, time::Instant};

use color_eyre::Result;
use wayper_windows::config::ResolvedImageContent;
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
    default_image: ResolvedImageContent,
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
    pub async fn new(default_image: ResolvedImageContent) -> Result<Self> {
        Ok(Self {
            renderer: Renderer::new().await?,
            outputs: Default::default(),
            window_id_iden_map: Default::default(),
            default_image,
        })
    }

    pub fn has_output(&self, output_iden: &str) -> bool {
        self.outputs.contains_key(output_iden)
    }

    pub fn get_window(&self, output_iden: &str) -> Option<&Arc<Window>> {
        self.outputs.get(output_iden).map(|output| &output.window)
    }

    pub fn output_id_for_window(&self, window_id: &WindowId) -> Option<&String> {
        self.window_id_iden_map.get(window_id)
    }

    pub fn add_output(
        &mut self,
        output_iden: String,
        window: Arc<Window>,
        size: PhysicalSize<u32>,
    ) -> Result<()> {
        self.renderer.create_surface(
            output_iden.clone(),
            (size.width, size.height),
            window.clone(),
        )?;

        self.window_id_iden_map
            .insert(window.id(), output_iden.clone());
        self.outputs.insert(
            output_iden,
            OutputState {
                window,
                size,
                player: Player::image(
                    self.default_image.path.clone(),
                    self.default_image.fit.into(),
                ),
                dirty: true,
            },
        );
        Ok(())
    }

    pub fn resize_output(&mut self, output_iden: &str, new_size: PhysicalSize<u32>) -> Result<()> {
        if let Some(output) = self.outputs.get_mut(output_iden) {
            output.size = new_size;
            output.dirty = true;
        }
        self.renderer
            .resize_surface(output_iden, (new_size.width, new_size.height))
    }

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
