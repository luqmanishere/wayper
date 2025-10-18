use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    reexports::{
        calloop,
        client::{self, Proxy},
    },
    shell::{WaylandSurface, wlr_layer::LayerShellHandler},
};
use tracing::{debug, error, info, instrument, trace, warn};
use wayper_lib::event_source::DrawSource;

use crate::{handlers::Wayper, map::OutputKey};

impl LayerShellHandler for Wayper {
    fn closed(
        &mut self,
        _conn: &client::Connection,
        _qh: &client::QueueHandle<Self>,
        _layer: &smithay_client_toolkit::shell::wlr_layer::LayerSurface,
    ) {
        tracing::debug!(
            "layer shell handler closed called for layer for surface {}",
            _layer.wl_surface().id()
        );
    }

    #[instrument(skip_all, fields(layer_id=layer.wl_surface().id().protocol_id()))]
    fn configure(
        &mut self,
        _conn: &client::Connection,
        _qh: &client::QueueHandle<Self>,
        layer: &smithay_client_toolkit::shell::wlr_layer::LayerSurface,
        configure: smithay_client_toolkit::shell::wlr_layer::LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let surface_id = layer.wl_surface().id();
        let (new_width, new_height) = configure.new_size;
        debug!(
            "received configure for {} with size: {}x{}",
            surface_id, new_width, new_height
        );

        let output = self
            .outputs
            .get(OutputKey::SurfaceId(surface_id.clone()))
            .expect("output initialized");

        {
            let output_handle = output.clone();
            let mut output_guard = output.lock().unwrap();
            if let Some(output_config) = output_guard.output_config.clone() {
                if output_guard.first_configure {
                    let output_id = output_guard.output_info.id;
                    info!("first configure for surface {}", surface_id);
                    output_guard.dimensions = Some(configure.new_size);

                    let output_name = &output_guard.output_name;

                    // Configure the wgpu surface for this output
                    let surface_format = match self
                        .wgpu
                        .configure_surface(output_name, (new_width, new_height))
                    {
                        Ok(format) => format,
                        Err(e) => {
                            error!("Failed to configure surface: {}", e);
                            return;
                        }
                    };

                    // Initialize the render pipeline if not already done
                    if let Err(e) = self.wgpu.init_image_pipeline(surface_format) {
                        error!("Failed to initialize image pipeline: {}", e);
                        return;
                    }

                    // first render
                    let first_image = output_guard.current_img();
                    if let Some(image_path) = first_image
                        && let Err(e) = self.wgpu.render_to_output(output_name, &image_path)
                    {
                        error!("Failed to render initial image: {}", e);
                    }

                    let next_image = output_guard.peek_next_img();
                    if let Err(e) = self.wgpu.request_texture_load(
                        &next_image,
                        (new_width, new_height),
                        output_name.to_string(),
                    ) {
                        error!("Failed to request pre-load of next image: {}", e);
                    }

                    layer.wl_surface().frame(_qh, layer.wl_surface().clone());
                    layer.wl_surface().commit();
                    debug!("finished configure, frame queued");

                    let dur = Duration::from_secs(output_config.duration.unwrap_or(60));
                    let (draw_source, ping_handle) =
                        DrawSource::from_duration(dur).expect("draw source can be initialized");

                    ping_handle.ping();
                    output_guard.ping_draw = Some(ping_handle);
                    output_guard.first_configure = false;

                    let draw_token = self
                        .c_queue_handle
                        .insert_source(draw_source, move |previous_deadline, _, _data| {
                            let instant = Instant::now();
                            let previous_deadline = previous_deadline.get_last_deadline();
                            let new_instant = previous_deadline + dur;

                            trace!(
                                "timer reached deadline: {:?} | new instant: {:?}",
                                previous_deadline, new_instant
                            );

                            output_handle.lock().unwrap().should_next = true;

                            tracing::debug!(
                                "processing time: {} ms",
                                (std::time::Instant::now() - instant).as_millis()
                            );

                            calloop::timer::TimeoutAction::ToInstant(new_instant)
                        })
                        .expect("draw timer initialized");
                    self.draw_tokens.insert(output_id, draw_token);
                } else if !output_guard.first_configure
                    && output_guard.dimensions != Some(configure.new_size)
                {
                    warn!("received configure event, screen size changed");
                    output_guard.dimensions = Some(configure.new_size);
                    // TODO: trigger redraw
                }
            } else {
                warn!(
                    "no configuration found for surface {}, output {}",
                    layer.wl_surface().id(),
                    output_guard.output_name
                );
            }
        }
    }
}
