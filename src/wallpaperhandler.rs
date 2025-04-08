use std::collections::HashMap;

use rand::seq::SliceRandom;
use smithay_client_toolkit::reexports::client;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            self,
            timer::{TimeoutAction, Timer},
            RegistrationToken,
        },
        client::{Proxy, QueueHandle},
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler},
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use tracing::{debug, error, info, instrument, trace, warn};
use walkdir::WalkDir;

use wayper::{
    config::WayperConfig,
    utils::{
        map::{OutputKey, OutputMap},
        output::OutputRepr,
        render_server::RenderServer,
    },
};

pub type OutputId = u32;
/// The key should be the output id from WlOutput
pub type TimerTokens = HashMap<OutputId, RegistrationToken>;

pub struct Wayper {
    pub compositor_state: CompositorState,
    pub registry_state: RegistryState,
    pub output_state: OutputState,
    pub layer_shell: LayerShell,
    pub shm: Shm,
    pub c_queue_handle: calloop::LoopHandle<'static, Self>,
    pub timer_tokens: TimerTokens,

    pub outputs: OutputMap,
    pub config: WayperConfig,
    pub socket_counter: u64,
    pub render_server: std::sync::Arc<RenderServer>,
    pub last_time: u32,
}

// TODO: modularize with calloop?

impl Wayper {
    pub fn draw(&mut self) {
        for rm in self.outputs.iter() {
            let mut v = rm.lock().unwrap();

            v.draw().expect("success drawing");
        }
    }

    pub fn add_output(
        &mut self,
        _conn: &client::Connection,
        qh: &client::QueueHandle<Self>,
        output: client::protocol::wl_output::WlOutput,
    ) {
        let output_info = self.output_state.info(&output).expect("get info");
        let outputs_map = &mut self.outputs;

        let name = output_info.name.clone().expect("output must have name");
        tracing::Span::current().record("name", &name);

        // if output does not exist we add it
        if !outputs_map.contains_key(OutputKey::OutputName(name.clone())) {
            info!("got new_output {}", name);

            let surface = self.compositor_state.create_surface(qh);
            let layer = self.layer_shell.create_layer_surface(
                qh,
                surface.clone(),
                Layer::Background,
                Some("wayper"),
                Some(&output),
            );

            // additional layer configuration
            layer.set_layer(Layer::Background);
            layer.set_size(0, 0);
            layer.set_exclusive_zone(-1);
            layer.set_anchor(Anchor::all());
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);

            // commit the layer
            layer.commit();

            let pool = SlotPool::new(256 * 256 * 4, &self.shm).expect("Failed to create pool");

            // no config no problem
            let output_config = match self.config.get_output_config(&name) {
                Ok(config) => Some(config),
                Err(e) => {
                    error!("Unable to get config for output: {e}");
                    None
                }
            };

            let img_list = if let Some(output_config) = output_config.as_ref() {
                if output_config.path.as_ref().unwrap().is_dir() {
                    let mut files = WalkDir::new(output_config.path.as_ref().unwrap())
                        .into_iter()
                        .filter(|e| {
                            mime_guess::from_path(e.as_ref().unwrap().path())
                                .iter()
                                .any(|ev| ev.type_() == "image")
                        })
                        .map(|e| e.unwrap().path().to_owned())
                        .collect::<Vec<_>>();

                    let mut rng = rand::rng();
                    files.shuffle(&mut rng);
                    debug!("{:?}", &files);
                    files
                } else {
                    vec![]
                }
            } else {
                vec![]
            };

            outputs_map.insert(
                name.clone(),
                surface.id(),
                output.id(),
                OutputRepr {
                    output_name: name.clone(),
                    wl_repr: output,
                    output_info,
                    output_config,
                    dimensions: None,
                    scale_factor: 1,
                    pool,
                    surface: Some(surface),
                    layer,
                    buffer: None,
                    first_configure: true,
                    img_list,
                    index: 0,
                    visible: true,
                    render_server: std::sync::Arc::clone(&self.render_server),
                    frame_count: 1,
                },
            );
        } else {
            info!("we had this output {name} earlier, skipping....");
        }

        // reassign the hashmap we take (took)
        // self.outputs = Some(outputs_hashmap);
    }
}

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
        let dt = (time as f64 - self.last_time as f64);
        debug!(
            "frame called for surface {:?}, time: {}, last_time: {}, dt: {dt}",
            surface, time, self.last_time
        );
        debug!("fps: {}", 1_f64 / dt * 1000_f64);

        // surface.frame(_qh, surface.clone());
        // surface.commit();
        self.last_time = time;
        // self.draw();
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

impl OutputHandler for Wayper {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    #[instrument(skip_all, fields(name))]
    fn new_output(
        &mut self,
        _conn: &client::Connection,
        qh: &client::QueueHandle<Self>,
        output: client::protocol::wl_output::WlOutput,
    ) {
        debug!("received new_output {} on output handler", output.id());
        self.add_output(_conn, qh, output);
    }

    fn update_output(
        &mut self,
        conn: &client::Connection,
        qh: &client::QueueHandle<Self>,
        output: client::protocol::wl_output::WlOutput,
    ) {
        // TODO: implement this because usecase is found - when an output is added
        debug!("received update_output for output {}", output.id());
        self.add_output(conn, qh, output);
    }

    fn output_destroyed(
        &mut self,
        _conn: &client::Connection,
        _qh: &client::QueueHandle<Self>,
        output: client::protocol::wl_output::WlOutput,
    ) {
        let info = self.output_state.info(&output).expect("output has info");
        output.release();
        let name = info.name.expect("output has name");

        let _removed = self.outputs.remove(OutputKey::OutputName(name.clone()));
        info!("output {name} was removed");
        match self.timer_tokens.remove_entry(&info.id) {
            Some((_, token)) => {
                self.c_queue_handle.remove(token);
                trace!("removed timer for {name}");
            }
            None => {
                error!("failed to remove timer_token entry");
            }
        }
    }
}

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
        // this function is called on instantiate and when there are dimension changes
        tracing::info!("received configure for {}", layer.wl_surface().id());
        let surface_id = layer.wl_surface().id();
        let output = self
            .outputs
            .get(OutputKey::SurfaceId(layer.wl_surface().id()))
            .expect("entry initialized");
        let output_handle = output.clone();
        {
            let mut output_guard = output.lock().unwrap();
            if let Some(output_config) = output_guard.output_config.clone() {
                if output_guard.first_configure {
                    let output_id = output_guard.output_info.id;
                    info!("first configure for surface {surface_id}");
                    output_guard.dimensions = Some(configure.new_size);

                    self.render_server
                        .submit_job(wayper::utils::render_server::RenderJobRequest::Video {
                            width: configure.new_size.0,
                            height: configure.new_size.1,
                            frame_count: 0,
                            video: "/home/luqman/wallpapers/notseiso/horizontal/live/starnyx_seele_live.mp4".into(),
                        })
                        .unwrap();
                    // first draw
                    layer.wl_surface().frame(_qh, layer.wl_surface().clone());
                    output_guard.draw().expect("success draw");

                    // copy the output to another thread, then send messages through channels
                    let timer = Timer::from_duration(std::time::Duration::from_secs(
                        output_config.duration.unwrap_or(60),
                    ));

                    let timer_token = self
                        .c_queue_handle
                        .insert_source(timer, move |previous_deadline, _, _data| {
                            // regardless of rendering time, the next deadline will be exactly
                            // n seconds later
                            let new_instant = previous_deadline
                                + std::time::Duration::from_secs(
                                    output_config.duration.unwrap_or(60),
                                );
                            trace!(
                                "timer reached deadline: {:?} | new instant: {:?}",
                                previous_deadline,
                                new_instant
                            );

                            // let surface = output_handle
                            //     .lock()
                            //     .unwrap()
                            //     .surface
                            //     .as_ref()
                            //     .unwrap()
                            //     .clone();
                            // surface.frame(&qh, surface.clone());

                            match output_handle.lock().unwrap().draw() {
                                Ok(_) => {
                                    tracing::info!("draw success");
                                }
                                Err(e) => {
                                    tracing::error!("draw failed with error: {e}");
                                }
                            }

                            TimeoutAction::ToInstant(new_instant)
                        })
                        .expect("works");
                    self.timer_tokens.insert(output_id, timer_token);
                    tracing::info!("done with configure");
                    // TODO: watch dir to update config
                } else if !output_guard.first_configure
                    && output_guard.dimensions != Some(configure.new_size)
                {
                    warn!("received configure event, screen size changed");
                    output_guard.dimensions = Some(configure.new_size);
                    output_guard.draw().expect("success draw");
                } else if !output_guard.first_configure
                    && output_guard.dimensions == Some(configure.new_size)
                {
                    warn!("received configure event, no size changes")
                }
            }
        }
    }
}

impl ShmHandler for Wayper {
    fn shm_state(&mut self) -> &mut smithay_client_toolkit::shm::Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Wayper {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_compositor!(Wayper);
delegate_output!(Wayper);
delegate_layer!(Wayper);
delegate_registry!(Wayper);
delegate_shm!(Wayper);
