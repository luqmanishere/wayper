use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use color_eyre::config;
use rand::seq::SliceRandom;
use smithay_client_toolkit::reexports::{calloop::ping, client};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{self, RegistrationToken},
        client::{Proxy, QueueHandle},
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler},
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use tracing::{debug, error, info, instrument, trace, warn};
use walkdir::WalkDir;

use wayper_lib::{config::Config, event_source::DrawSource};

use crate::{
    map::{OutputKey, OutputMap},
    output::OutputRepr,
};
use crate::{
    render_server::{RenderJobRequest, RenderServer},
    wgpu_renderer::WgpuRenderer,
};

pub type OutputId = u32;
/// The key should be the output id from WlOutput
pub type DrawTokens = HashMap<OutputId, RegistrationToken>;

pub struct Wayper {
    pub compositor_state: CompositorState,
    pub registry_state: RegistryState,
    pub output_state: OutputState,
    pub layer_shell: LayerShell,
    pub shm: Shm,
    pub c_queue_handle: calloop::LoopHandle<'static, Self>,
    pub draw_tokens: DrawTokens,

    pub current_profile: String,
    pub outputs: OutputMap,
    pub config: Config,
    pub socket_counter: u64,
    pub render_server: std::sync::Arc<RenderServer>,

    pub wgpu: WgpuRenderer,
}

// TODO: modularize with calloop?

impl Wayper {
    #[expect(dead_code)]
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
            self.wgpu
                .new_surface(
                    name.clone(),
                    _conn.backend().display_ptr(),
                    layer.wl_surface().id().as_ptr(),
                )
                .unwrap();

            // no config no problem
            let output_config = match self
                .config
                .get_output_config(&*self.current_profile, name.as_str())
            {
                Ok(config) => Some(config),
                Err(e) => {
                    error!("Unable to get config for output: {e}");
                    None
                }
            };

            let img_list = get_img_list(output_config.as_ref());

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
                    ping_draw: None,
                    img_list,
                    index: 0,
                    visible: true,
                    should_next: false,
                    last_render_instant: Instant::now(),
                    render_server: std::sync::Arc::clone(&self.render_server),
                },
            );
        } else {
            warn!("we had this output {name} earlier, skipping....");
        }
    }
    pub fn change_profile<P>(&mut self, profile: P) -> color_eyre::Result<String>
    where
        P: Into<Option<String>>,
    {
        let profile =
            Into::<Option<String>>::into(profile).unwrap_or(self.config.default_profile.clone());
        if !&self
            .config
            .profiles
            .profiles()
            .contains(&profile.to_string())
        {
            return Err(color_eyre::eyre::eyre!("Profile does not exist"));
        }

        if profile == self.current_profile {
            warn!(
                "Not changing to currently active profile {}",
                self.current_profile
            );
            return Ok(profile);
        }

        info!("Changing current profile to: \"{profile}\"");

        // set the profile
        self.current_profile = profile.to_string();

        // refresh the img list
        for output in self.outputs.iter() {
            let output_name = output.lock().unwrap().output_name.clone();

            let output_config = self
                .config
                .get_output_config(profile.as_str(), &*output_name)?;

            let mut output = output.lock().unwrap();
            output.img_list = get_img_list(Some(&output_config));
            output.index = 0;
            output.output_config = Some(output_config);
            if let Some(ping_draw) = output.ping_draw.as_ref() {
                ping_draw.ping();
            } else {
                // incase ping_draw doesnt exist, which should not happen after the first configure
                let (width, height) = output.dimensions.unwrap_or_default();
                // set first configure to get the first image
                output.first_configure = true;
                let image = output.peek_next_img();
                output.render_server.submit_job(RenderJobRequest::Image {
                    width,
                    height,
                    image,
                })?;
            }
        }

        Ok(profile)
    }
}

/// Get a list of images from a config
fn get_img_list(
    output_config: Option<&wayper_lib::config::OutputConfig>,
) -> Vec<std::path::PathBuf> {
    if let Some(output_config) = output_config {
        if output_config.path.is_dir() {
            let mut files = WalkDir::new(&output_config.path)
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
        trace!("frame called {:?} - {}", surface, time);
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
        match self.draw_tokens.remove_entry(&info.id) {
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
                        .configure_surface(&output_name, (new_width, new_height))
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
                    if let Some(image_path) = first_image {
                        if let Err(e) = self.wgpu.render_to_output(&output_name, &image_path) {
                            error!("Failed to render initial image: {}", e);
                        }
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
                        .insert_source(draw_source, move |previous_deadline, _, data| {
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

        /*
        // this function is called on instantiate and when there are dimension changes
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

                    // first draw
                    // output_guard.draw().expect("success draw");

                    // timer for calloop
                    let (draw_source, ping_handle) = DrawSource::from_duration(
                        std::time::Duration::from_secs(output_config.duration.unwrap_or(60)),
                    )
                    .expect("initiatable");

                    ping_handle.ping();
                    output_guard.ping_draw = Some(ping_handle);

                    let draw_token = self
                        .c_queue_handle
                        .insert_source(draw_source, move |previous_deadline, _, _data| {
                            let instant = std::time::Instant::now();
                            // regardless of rendering time, the next deadline will be exactly
                            // n seconds later
                            let previous_deadline = previous_deadline.get_last_deadline();
                            let new_instant = previous_deadline
                                + std::time::Duration::from_secs(
                                    output_config.duration.unwrap_or(60),
                                );
                            trace!(
                                "timer reached deadline: {:?} | new instant: {:?}",
                                previous_deadline, new_instant
                            );

                            let mut lock = output_handle.lock().unwrap();
                            match lock.draw() {
                                Ok(path) => {
                                    tracing::info!("draw success");
                                    if let Some(config) = &lock.output_config
                                        && let Some(command) = &config.run_command
                                    {
                                        let mut command =
                                            shlex::Shlex::new(command).collect::<Vec<_>>();

                                        // drop immediately
                                        drop(lock);

                                        // rudimentary substitution that I can't figure out how to do in place
                                        for arg in command.iter_mut() {
                                            if arg == "{image}" {
                                                arg.clear();
                                                arg.push_str(&path.display().to_string());
                                            }
                                        }

                                        tracing::info!("running command {}", command.join(" "));

                                        // let chains wooooo
                                        if let Some((command, args)) = command.split_first()
                                            && let Ok((mut pipe_reader, pipe_writer)) =
                                                std::io::pipe()
                                            && let Ok(mut  child) =
                                                std::process::Command::new(command)
                                                    .args(args)
                                                    .stderr(
                                                        pipe_writer
                                                            .try_clone()
                                                            .expect("pipe writer cannot be cloned"),
                                                    )
                                                    .stdout(pipe_writer)
                                                    .spawn()
                                        {
                                            std::thread::spawn(move || {
                                                let mut buf = String::new();
                                                pipe_reader
                                                    .read_to_string(&mut buf)
                                                    .expect("readable pipe");
                                                let buf = buf.trim();

                                                if !buf.is_empty() {
                                                    tracing::warn!("color_command output:\n{buf}");
                                                }

                                                if let Ok(exit_status) = child.wait()
                                                    && !exit_status.success()
                                                {
                                                    tracing::error!(
                                                        "command exited with code {:?}",
                                                        exit_status.code()
                                                    );
                                                } else {
                                                    tracing::info!("command executed successfully");
                                                }
                                            });
                                        } else {
                                            tracing::error!("command run error, check if the command exists and is correct");
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("draw failed with error: {e}");
                                }
                            }
                            tracing::debug!("processing time: {} ms", (std::time::Instant::now() - instant).as_millis());

                            TimeoutAction::ToInstant(new_instant)
                        })
                        .expect("works");
                    self.draw_tokens.insert(output_id, draw_token);
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
        */
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
