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

    #[tracing::instrument(skip_all, fields(surface_id = %surface.id(), time = time))]
    fn frame(
        &mut self,
        _conn: &client::Connection,
        _qh: &client::QueueHandle<Self>,
        surface: &client::protocol::wl_surface::WlSurface,
        time: u32,
    ) {
        // TODO: visibility via logs. considering slog
        trace!("frame called");

        let process_start = Instant::now();
        if let Ok(count) = self.wgpu.process_loaded_textures()
            && count > 0
        {
            let process_time = process_start.elapsed();
            debug!(
                count = count,
                time_us = process_time.as_micros(),
                "Processed pre-loaded textures"
            );
        }

        let surface_id = surface.id();

        if let Some(output) = self.outputs.get(OutputKey::SurfaceId(surface_id.clone())) {
            let mut output_handle = output.lock().unwrap();

            if let Some(transition) = &mut output_handle.transition {
                transition.start();

                // rip any compositors that can't handle this
                if !transition.should_render_frame() {
                    trace!("Frame skipped - FPS throttle");
                    surface.frame(_qh, surface.clone());
                    surface.commit();
                    return;
                }

                // i wonder if this is good practice or nah?
                let progress = transition.progress();
                let eased_progress = transition.eased_progress();
                let transition_type = transition.transition_type.to_u32();
                let is_complete = transition.is_complete();
                let transition_elapsed = transition.start_time
                    .map(|t| t.elapsed().as_millis())
                    .unwrap_or(0);
                let output_name = output_handle.output_name.clone();

                trace!(
                    progress = progress,
                    eased_progress = eased_progress,
                    elapsed_ms = transition_elapsed,
                    "Transition frame"
                );

                let previous_img = output_handle.previous_img();
                let Some(current_img) = output_handle.current_img() else {
                    error!("no current image found for {}", output_name);
                    return;
                };

                let render_start = Instant::now();
                if let Err(e) = self.wgpu.render_frame(
                    &output_name,
                    previous_img.as_deref(),
                    &current_img,
                    eased_progress,
                    transition_type,
                ) {
                    error!("failed to render transition frame: {e}");
                }
                let render_time = render_start.elapsed();
                trace!(render_time_us = render_time.as_micros(), "GPU render time");

                if is_complete {
                    let current_index = output_handle.index;
                    let current_image = output_handle.current_img();

                    info!(
                        "{} showing [{}] {}",
                        output_name,
                        current_index,
                        current_image.as_ref()
                            .and_then(|p| p.file_name())
                            .and_then(|n| n.to_str())
                            .unwrap_or("unknown")
                    );

                    debug!("Transition complete for {}", output_name);
                    output_handle.transition = None;
                    output_handle.last_render_instant = Instant::now();
                    output_handle.frame_count += 1;

                    // Log GPU metrics every 100 frames
                    if output_handle.frame_count % 100 == 0 {
                        drop(output_handle);
                        self.wgpu.log_gpu_metrics();
                        output_handle = output.lock().unwrap();
                    }

                    if output_handle.first_configure {
                        output_handle.first_configure = false;
                    }
                }

                surface.frame(_qh, surface.clone());
                surface.commit();
            } else if output_handle.should_next {
                let last_render = output_handle.last_render_instant.elapsed();
                let output_age = output_handle.created_at.elapsed();
                let frame_count = output_handle.frame_count;
                debug!(
                    "last render was {:.3}s ago | output age: {:.3}s | frame: {}",
                    last_render.as_secs_f64(),
                    output_age.as_secs_f64(),
                    frame_count
                );

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

                    let transition_name = match transition_type {
                        wayper_lib::config::TransitionType::Crossfade => "crossfade",
                    };

                    info!(
                        "{} transitioning with {}",
                        output_handle.output_name,
                        transition_name
                    );

                    debug!(
                        "Starting {} transition for {} (duration: {}ms, {} FPS)",
                        transition_name,
                        output_handle.output_name,
                        duration_ms,
                        target_fps
                    );

                    surface.frame(_qh, surface.clone());
                } else {
                    let current_index = output_handle.index;
                    let output_name = output_handle.output_name.clone();

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

                    let filename = image.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown");

                    info!(
                        "{} showing [{}] {}",
                        output_name,
                        current_index,
                        filename
                    );

                    output_handle.last_render_instant = Instant::now();
                    output_handle.frame_count += 1;

                    // Log GPU metrics every 100 frames
                    if output_handle.frame_count % 100 == 0 {
                        drop(output_handle);
                        self.wgpu.log_gpu_metrics();
                        output_handle = output.lock().unwrap();
                    }
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
