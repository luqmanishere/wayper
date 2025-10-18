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
        trace!("frame called {:?} - {}", surface, time);

        if let Ok(count) = self.wgpu.process_loaded_textures()
            && count > 0
        {
            debug!("Processed {} pre-loaded textures", count);
        }

        let surface_id = surface.id();

        if let Some(output) = self.outputs.get(OutputKey::SurfaceId(surface_id.clone())) {
            let mut output_handle = output.lock().unwrap();

            // all this is temp since we will start with animations soonish.
            if output_handle.should_next {
                let last_render = output_handle.last_render_instant.elapsed();
                tracing::debug!("last render was {}s ago", last_render.as_secs_f64());

                output_handle.next();

                let Some(image) = output_handle.current_img() else {
                    error!("no image found for {}", output_handle.output_name);
                    return;
                };

                if let Err(e) = self
                    .wgpu
                    .handle_frame(&output_handle.output_name, image.as_path())
                {
                    error!("failed to render frame: {e}");
                }
                output_handle.should_next = !output_handle.should_next;
                output_handle.last_render_instant = Instant::now();

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
            }
        } else {
            error!("no output configured for surface {surface_id}");
        }

        // remember to request other frames
        surface.frame(_qh, surface.clone());
        surface.commit();
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
