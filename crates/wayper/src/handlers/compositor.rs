use std::time::Instant;

use smithay_client_toolkit::{
    compositor::CompositorHandler,
    reexports::client::{self, Proxy, QueueHandle},
};
use tracing::{debug, error, info, trace};

use crate::{
    handlers::{Wayper, utils},
    map::OutputKey,
};

impl CompositorHandler for Wayper {
    fn scale_factor_changed(
        &mut self,
        _conn: &client::Connection,
        _qh: &client::QueueHandle<Self>,
        surface: &client::protocol::wl_surface::WlSurface,
        new_factor: i32,
    ) {
        debug!(
            "scale factor changed for surface {:?} - {}",
            surface.id(),
            new_factor
        );
    }

    fn frame(
        &mut self,
        _conn: &client::Connection,
        _qh: &client::QueueHandle<Self>,
        surface: &client::protocol::wl_surface::WlSurface,
        time: u32,
    ) {
        // TODO: visibility via logs. considering slog
        trace!("frame called {:?} - {}", surface, time);

        if let Ok(count) = self.wgpu.process_loaded_textures()
            && count > 0
        {
            debug!("Processed {} pre-loaded textures", count);
        }

        let surface_id = surface.id();

        if let Some(output) = self.outputs.get(OutputKey::SurfaceId(surface_id.clone())) {
            let mut output_handle = output.lock().unwrap();

            if let Some(transition) = &mut output_handle.transition {
                transition.start();

                // rip any compositors that can't handle this
                if !transition.should_render_frame() {
                    surface.frame(_qh, surface.clone());
                    surface.commit();
                    return;
                }

                // i wonder if this is good practice or nah?
                let progress = transition.eased_progress();
                let transition_type = transition.transition_type.to_u32();
                let is_complete = transition.is_complete();
                let output_name = output_handle.output_name.clone();

                let previous_img = output_handle.previous_img();
                let Some(current_img) = output_handle.current_img() else {
                    error!("no current image found for {}", output_name);
                    return;
                };

                if let Err(e) = self.wgpu.render_frame(
                    &output_name,
                    previous_img.as_deref(),
                    &current_img,
                    progress,
                    transition_type,
                ) {
                    error!("failed to render transition frame: {e}");
                }

                if is_complete {
                    debug!("Transition complete for {}", output_name);
                    output_handle.transition = None;
                    output_handle.last_render_instant = Instant::now();

                    if output_handle.first_configure {
                        output_handle.first_configure = false;
                    }
                }

                surface.frame(_qh, surface.clone());
                surface.commit();
            } else if output_handle.should_next {
                let last_render = output_handle.last_render_instant.elapsed();
                debug!("last render was {}s ago", last_render.as_secs_f64());

                output_handle.next();

                let Some(image) = output_handle.current_img() else {
                    error!("no image found for {}", output_handle.output_name);
                    return;
                };

                let should_animate = if let Some(config) = &output_handle.output_config {
                    config.is_transitions_enabled(&self.config)
                } else {
                    false
                };

                if should_animate {
                    let config = output_handle.output_config.as_ref().unwrap();
                    let duration_ms = config.get_transition_duration(&self.config);
                    let target_fps = config.get_transition_fps(&self.config);
                    let transition_type = config.get_transition_type();

                    output_handle.transition = Some(crate::output::TransitionData::new(
                        transition_type,
                        duration_ms,
                        target_fps,
                    ));

                    info!(
                        "Starting {} transition for {} at {} FPS",
                        match transition_type {
                            wayper_lib::config::TransitionType::Crossfade => "crossfade",
                        },
                        output_handle.output_name,
                        target_fps
                    );

                    surface.frame(_qh, surface.clone());
                } else {
                    let previous_img = output_handle.previous_img();
                    if let Err(e) = self.wgpu.render_frame(
                        &output_handle.output_name,
                        previous_img.as_deref(),
                        image.as_path(),
                        1.0,
                        0,
                    ) {
                        error!("failed to render frame: {e}");
                    }
                    output_handle.last_render_instant = Instant::now();
                }

                output_handle.should_next = false;

                let next_image = output_handle.peek_next_img();
                if let Some(dims) = output_handle.dimensions
                    && let Err(e) = self.wgpu.request_texture_load(
                        &next_image,
                        dims,
                        output_handle.output_name.clone(),
                    )
                {
                    error!("Failed to request pre-load: {}", e);
                }

                if let Some(config) = &output_handle.output_config
                    && let Some(command) = config.run_command.clone()
                {
                    let img_path = image.clone();
                    std::thread::spawn(|| utils::run_command(command, img_path));
                }

                surface.commit();
            } else {
                surface.frame(_qh, surface.clone());
                surface.commit();
            }
        } else {
            error!("no output configured for surface {surface_id}");
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &client::Connection,
        _qh: &QueueHandle<Self>,
        surface: &client::protocol::wl_surface::WlSurface,
        new_transform: client::protocol::wl_output::Transform,
    ) {
        debug!(
            "{:?} - received new transform - {:?}",
            surface, new_transform
        );
    }
    fn surface_enter(
        &mut self,
        _conn: &client::Connection,
        _qh: &QueueHandle<Self>,
        _surface: &client::protocol::wl_surface::WlSurface,
        output: &client::protocol::wl_output::WlOutput,
    ) {
        info!("surface enter for output {}", output.id());
    }
    fn surface_leave(
        &mut self,
        _conn: &client::Connection,
        _qh: &QueueHandle<Self>,
        _surface: &client::protocol::wl_surface::WlSurface,
        output: &client::protocol::wl_output::WlOutput,
    ) {
        info!("surface leave for output {}", output.id());
    }
}
