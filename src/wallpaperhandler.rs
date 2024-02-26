use std::{
    collections::HashMap,
    io::{BufWriter, Write},
    path::PathBuf,
};

use eyre::Result;
use image::imageops::FilterType;
use rand::seq::SliceRandom;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputInfo, OutputState},
    reexports::{
        calloop::{
            self,
            timer::{TimeoutAction, Timer},
            RegistrationToken,
        },
        client::{
            self,
            protocol::{wl_output::WlOutput, wl_shm, wl_surface::WlSurface},
            Proxy, QueueHandle,
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use tracing::{debug, error, info, instrument, trace, warn};
use walkdir::WalkDir;

use crate::config::{OutputConfig, WayperConfig};

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

    pub outputs: Option<HashMap<String, OutputRepr>>,
    pub config: WayperConfig,
}

// TODO: modularize with calloop?

impl Wayper {
    pub fn draw(&mut self) {
        for (_k, v) in self.outputs.as_mut().expect("exists") {
            v.draw().expect("success drawing");
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct OutputRepr {
    output_name: String,
    wl_repr: WlOutput,
    output_info: OutputInfo,
    output_config: OutputConfig,
    dimensions: Option<(u32, u32)>,
    scale_factor: i64,
    qh: QueueHandle<Wayper>,
    first_configure: bool,

    pool: SlotPool,
    surface: Option<WlSurface>,
    layer: LayerSurface,

    index: usize,
    img_list: Vec<PathBuf>,
}

impl OutputRepr {
    #[instrument(skip_all, fields(name=self.output_name))]
    pub fn update_config(&mut self, new_config: OutputConfig) {
        trace!("new config: {new_config:?}");
        if new_config
            .name
            .as_ref()
            .expect("config must have output name")
            == &self.output_name
        {
            self.output_config = new_config;

            info!("received updated config");
        }
    }

    #[instrument(skip_all, fields(name=self.output_name))]
    pub fn draw(&mut self) -> Result<()> {
        trace!("begin drawing");
        let (width, height) = self.dimensions.expect("exists");
        let stride = width as i32 * 4;

        let path = self.next();
        info!("drawing: {}", path.display());

        let image = image::open(&path)?
            .resize_to_fill(
                width.try_into().unwrap(),
                height.try_into().unwrap(),
                FilterType::Lanczos3,
            )
            .into_rgba8();

        let (buffer, canvas) = self
            .pool
            .create_buffer(
                width as i32,
                height as i32,
                stride,
                wl_shm::Format::Abgr8888,
            )
            .expect("create buffer");

        // Draw to the window:
        {
            let mut writer = BufWriter::new(canvas);
            writer.write_all(image.as_raw()).unwrap();
            writer.flush().unwrap();
        }

        // Damage the entire window
        self.layer
            .wl_surface()
            .damage_buffer(0, 0, width as i32, height as i32);

        // Attach and commit to present.
        buffer
            .attach_to(self.layer.wl_surface())
            .expect("buffer attach");
        self.layer.commit();

        // TODO: save and reuse buffer when the window size is unchanged.  This is especially
        // useful if you do damage tracking, since you don't need to redraw the undamaged parts
        // of the canvas.
        if self.first_configure == true {
            self.first_configure = false;
        }
        trace!("finish drawing");
        Ok(())
    }

    /// if there is an image on current_img, give the image and increase index
    fn next(&mut self) -> PathBuf {
        let img_list = &self.img_list;
        let index = self.index;
        debug!("Current index is {}", index);
        if index < img_list.len() - 1 {
            self.index = index + 1;
        }
        if index == img_list.len() - 1 {
            self.index = 0;
        }

        debug!("new index is {}", self.index);
        img_list[self.index].clone()
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
        debug!("{:?} - {}", surface, new_factor);
    }

    fn frame(
        &mut self,
        _conn: &client::Connection,
        _qh: &client::QueueHandle<Self>,
        surface: &client::protocol::wl_surface::WlSurface,
        time: u32,
    ) {
        debug!("{:?} - {}", surface, time);
        self.draw();
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
        let output_info = self.output_state.info(&output).expect("get info");
        let mut outputs_hashmap = if let Some(outputs_hashmap) = self.outputs.take() {
            outputs_hashmap
        } else {
            HashMap::new()
        };
        let name = output_info.name.clone().expect("output must have name");
        tracing::Span::current().record("name", &name);
        info!("got new_output");

        let surface = self.compositor_state.create_surface(&qh);
        let layer = self.layer_shell.create_layer_surface(
            &qh,
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

        let output_config = self
            .config
            .get_output_config(&name)
            .expect(format!("config for display {name} exists").as_str());

        let img_list = if output_config.path.as_ref().unwrap().is_dir() {
            let mut files = WalkDir::new(output_config.path.as_ref().unwrap())
                .into_iter()
                .filter(|e| {
                    mime_guess::from_path(e.as_ref().unwrap().path())
                        .iter()
                        .any(|ev| ev.type_() == "image")
                })
                .map(|e| e.unwrap().path().to_owned())
                .collect::<Vec<_>>();

            let mut rng = rand::thread_rng();
            files.shuffle(&mut rng);
            debug!("{:?}", &files);
            files
        } else {
            vec![]
        };

        outputs_hashmap.insert(
            name.clone(),
            OutputRepr {
                output_name: name.clone(),
                wl_repr: output,
                qh: qh.clone(),
                output_info,
                output_config,
                dimensions: None,
                scale_factor: 1,
                pool,
                surface: Some(surface),
                layer,
                first_configure: true,
                img_list,
                index: 0,
            },
        );
        self.outputs = Some(outputs_hashmap);
        info!("added new output to map");
    }

    fn update_output(
        &mut self,
        _conn: &client::Connection,
        _qh: &client::QueueHandle<Self>,
        _output: client::protocol::wl_output::WlOutput,
    ) {
        unimplemented!("please report this usecase");
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

        let outputs = self.outputs.as_mut().unwrap();
        match outputs.remove(&name) {
            Some(_) => {
                info!("output {name} was removed");
                match self.timer_tokens.remove_entry(&info.id) {
                    Some((_id, token)) => {
                        self.c_queue_handle.remove(token);
                        trace!("removed timer for {name}");
                    }
                    None => {
                        error!("failed to remove timer_token entry");
                    }
                };
            }
            None => {
                error!("failed to remove output {name}");
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
        let _id = layer.wl_surface().id().protocol_id();
        for (_, v) in self.outputs.as_mut().expect("has") {
            let surface_prot_id = v
                .surface
                .as_ref()
                .expect("surface exists")
                .id()
                .protocol_id();
            if surface_prot_id == layer.wl_surface().id().protocol_id() {
                if v.first_configure {
                    let output_id = v.output_info.id;
                    info!("first configure for surface {surface_prot_id}");
                    v.dimensions = Some(configure.new_size);
                    v.draw().expect("success draw");
                    // TODO: set up first timer
                    let timer = Timer::from_duration(std::time::Duration::from_secs(
                        v.output_config.duration.unwrap_or(60),
                    ));
                    let id = surface_prot_id.clone();
                    let timer_token = self
                        .c_queue_handle
                        .insert_source(timer, move |deadline, _, data| {
                            trace!("timer reached deadline: {}", deadline.elapsed().as_secs());
                            for (_, v) in data.outputs.as_mut().expect("has") {
                                let surface_prot_id = v
                                    .surface
                                    .as_ref()
                                    .expect("surface exists")
                                    .id()
                                    .protocol_id();
                                if surface_prot_id == id {
                                    //TODO: log failure
                                    v.draw().expect("draw success");
                                    return TimeoutAction::ToDuration(
                                        std::time::Duration::from_secs(
                                            v.output_config.duration.unwrap_or(60),
                                        ),
                                    );
                                }
                            }
                            return TimeoutAction::ToDuration(std::time::Duration::from_secs(60));
                        })
                        .expect("works");
                    self.timer_tokens.insert(output_id, timer_token);
                    // TODO: watch dir to update config
                } else if !v.first_configure && v.dimensions != Some(configure.new_size) {
                    warn!("received configure event, screen size changed");
                    v.dimensions = Some(configure.new_size);
                    v.draw().expect("success draw");
                } else if !v.first_configure && v.dimensions == Some(configure.new_size) {
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
