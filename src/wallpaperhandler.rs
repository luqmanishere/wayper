use std::{
    collections::HashMap,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::{mpsc::TryRecvError, Arc, Mutex},
    thread::JoinHandle,
    time::Duration,
};

use eyre::Result;
use image::imageops::FilterType;
use impl_tools::autoimpl;
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
    shm::{
        slot::{Buffer, SlotPool},
        Shm, ShmHandler,
    },
};
use tracing::{debug, error, info, instrument, trace, warn};
use video_rs::Options;
use walkdir::WalkDir;

use crate::config::{OutputConfig, WayperConfig};

pub type OutputId = u32;
/// The key should be the output id from WlOutput
pub type TimerTokens = HashMap<OutputId, RegistrationToken>;

pub enum RenderStatus {
    /// An image was rendered
    Image,
    /// A frame from a video was handled, loop for the next one
    Video,
    /// Final frame of the video was handled, do not loop
    VideoEnd,
    /// Rendering failed, loop back
    Fail,
    Empty,
}

pub enum Msg {
    Draw,
    Removed,
    None,
}

pub struct Wayper {
    pub compositor_state: CompositorState,
    pub registry_state: RegistryState,
    pub output_state: OutputState,
    pub layer_shell: LayerShell,
    pub shm: Shm,
    pub c_queue_handle: calloop::LoopHandle<'static, Self>,
    pub timer_tokens: TimerTokens,

    pub outputs_map: HashMap<String, Arc<Mutex<OutputRepr>>>,
    pub layershell_to_outputname: HashMap<u32, String>,
    pub thread_map: HashMap<String, JoinHandle<()>>,
    pub config: WayperConfig,
}

// TODO: modularize with calloop?

impl Wayper {
    pub fn add_output(
        &mut self,
        _conn: &client::Connection,
        qh: &client::QueueHandle<Self>,
        output: client::protocol::wl_output::WlOutput,
    ) {
        let output_info = self.output_state.info(&output).expect("get info");
        let outputs_hashmap = &mut self.outputs_map;

        let name = output_info.name.clone().expect("output must have name");
        tracing::Span::current().record("name", &name);

        // if output does not exist we add it
        if outputs_hashmap.get(&name).is_none() {
            info!("got new_output {}", name);

            let surface = self.compositor_state.create_surface(&qh);
            let layer_surface = self.layer_shell.create_layer_surface(
                &qh,
                surface.clone(),
                Layer::Background,
                Some("wayper"),
                Some(&output),
            );

            // additional layer configuration
            layer_surface.set_size(0, 0);
            layer_surface.set_exclusive_zone(-1);
            layer_surface.set_anchor(Anchor::all());
            layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);

            // commit the layer
            layer_surface.commit();

            let pool = SlotPool::new(256 * 256 * 4, &self.shm).expect("Failed to create pool");

            // no config no problem
            let output_config = match self.config.get_output_config(&name) {
                Ok(config) => Some(config),
                Err(e) => {
                    error!("Unable to get config for output: {e}");
                    None
                }
            };

            let mut animation = false;

            let img_list = if let Some(output_config) = output_config.as_ref() {
                animation = output_config.animated;
                if output_config.path.as_ref().unwrap().is_dir() {
                    let mut files = WalkDir::new(output_config.path.as_ref().unwrap())
                        .into_iter()
                        .filter(|e| {
                            mime_guess::from_path(e.as_ref().unwrap().path())
                                .iter()
                                .any(|ev| {
                                    if output_config.animated {
                                        ev.type_() == "image" || ev.type_() == "video"
                                    } else {
                                        ev.type_() == "image"
                                    }
                                })
                        })
                        .map(|e| {
                            let e = e.unwrap();
                            (
                                e.path().to_owned(),
                                mime_guess::from_path(e.path())
                                    .first()
                                    .unwrap()
                                    .type_()
                                    .as_str()
                                    .to_owned(),
                            )
                        })
                        .collect::<Vec<_>>();

                    let mut rng = rand::thread_rng();
                    files.shuffle(&mut rng);
                    debug!("{:?}", &files);
                    files
                } else {
                    let path = output_config.path.as_ref().unwrap();
                    let mime = mime_guess::from_path(path)
                        .first()
                        .unwrap()
                        .type_()
                        .as_str()
                        .to_owned();
                    vec![(path.to_owned(), mime)]
                }
            } else {
                vec![]
            };

            self.layershell_to_outputname
                .insert(layer_surface.wl_surface().id().protocol_id(), name.clone());

            outputs_hashmap.insert(
                name.clone(),
                Arc::new(Mutex::new(OutputRepr {
                    output_name: name.clone(),
                    wl_repr: output,
                    qh: qh.clone(),
                    animation,
                    output_info,
                    output_config,
                    dimensions: None,
                    scale_factor: 1,
                    pool,
                    surface: Some(surface),
                    layer: layer_surface,
                    buffer: None,
                    frame_called: false,
                    first_configure: true,
                    file_list: img_list,
                    index: 0,
                    decoder: None,
                })),
            );
        } else {
            info!("we had this output {name} earlier, skipping....");
        }
    }
}

#[autoimpl(Debug ignore self.decoder)]
pub struct OutputRepr {
    output_name: String,
    #[allow(dead_code)]
    wl_repr: WlOutput,
    output_info: OutputInfo,
    output_config: Option<OutputConfig>,
    dimensions: Option<(u32, u32)>,
    #[allow(dead_code)]
    scale_factor: i64,
    #[allow(dead_code)]
    qh: QueueHandle<Wayper>,
    first_configure: bool,
    animation: bool,

    pool: SlotPool,
    buffer: Option<Buffer>,
    surface: Option<WlSurface>,
    frame_called: bool,
    layer: LayerSurface,

    decoder: Option<video_rs::Decoder>,

    index: usize,
    file_list: Vec<(PathBuf, String)>,
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
            self.output_config = Some(new_config);
            self.buffer = None;

            info!("received updated config");
        }
    }

    #[instrument(skip_all, fields(name=self.output_name, layer_id=self.layer.wl_surface().id().protocol_id()))]
    pub fn draw(
        &mut self,
        last_render_status: RenderStatus,
        timer_called: bool,
    ) -> Result<RenderStatus> {
        trace!("begin drawing");
        if self.frame_called || self.first_configure {
            match last_render_status {
                RenderStatus::Empty | RenderStatus::Image | RenderStatus::VideoEnd => {
                    if timer_called && !self.first_configure {
                        self.next();
                    }

                    let (path, filetype) = self.img();
                    let file = image::io::Reader::open(&path)?;
                    let file_format = file.format();

                    if filetype == "image" {
                        debug!("file format is {:?}", file_format);

                        self.draw_img(path)?;
                        return Ok(RenderStatus::Image);
                    } else if filetype == "video" {
                        self.draw_vid(path)?;
                    } else {
                        self.next();
                    }
                    return Ok(RenderStatus::Empty);
                }

                _ => Ok(RenderStatus::Empty),
            }
        } else {
            debug!("frame callback is not called, requesting a new one");
            self.layer
                .wl_surface()
                .frame(&self.qh, self.layer.wl_surface().clone());
            self.layer.commit();
            // return the last render status
            Ok(last_render_status)
        }
    }

    #[instrument(skip_all, fields(name=self.output_name, layer_id=self.layer.wl_surface().id().protocol_id()))]
    fn draw_img(&mut self, path: PathBuf) -> Result<()> {
        let (width, height) = self.dimensions.expect("exists");
        let stride = width as i32 * 4;

        info!("drawing: {}", path.display());

        let image = image::io::Reader::open(&path)?
            .decode()?
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

        // send a frame request
        self.layer
            .wl_surface()
            .frame(&self.qh, self.layer.wl_surface().clone());

        // Attach and commit to present.
        buffer
            .attach_to(self.layer.wl_surface())
            .expect("buffer attach");
        self.layer.commit();

        // reuse the buffer created, since
        self.buffer = Some(buffer);

        if self.first_configure {
            self.first_configure = false;
        }
        trace!("finish drawing");

        self.frame_called = false;
        return Ok(());
    }

    fn draw_vid(&mut self, path: PathBuf) -> Result<RenderStatus> {
        let (width, height) = self.dimensions.expect("exists");
        let stride = width as i32 * 4;
        let mut decoder = match self.decoder.take() {
            Some(decoder) => decoder,
            None => video_rs::Decoder::new_with_options_and_resize(
                &path.into(),
                &Options::default(),
                video_rs::Resize::Fit(width, height),
            )?,
        };
        decoder.frame_rate();

        let frame = decoder.decode_raw()?;
        let mut frame2 = frame.clone();
        frame
            .converter(video_rs::ffmpeg::format::Pixel::RGB565)?
            .run(&frame, &mut frame2)?;
        // let rgb = frame.slice(ndarray::s![0, 0, ..]).to_slice().unwrap();
        let data = frame.data(0);
        let (buffer, canvas) = self
            .pool
            .create_buffer(width as i32, height as i32, stride, wl_shm::Format::Rgb565)
            .expect("create buffer");
        println!("{}", data.len());

        // Draw to the window:
        {
            let mut writer = BufWriter::new(canvas);
            writer.write_all(data).unwrap();
            writer.flush().unwrap();
        }

        // Damage the entire window
        self.layer
            .wl_surface()
            .damage_buffer(0, 0, width as i32, height as i32);

        // send a frame request
        self.layer
            .wl_surface()
            .frame(&self.qh, self.layer.wl_surface().clone());

        // Attach and commit to present.
        buffer
            .attach_to(self.layer.wl_surface())
            .expect("buffer attach");
        self.layer.commit();

        // reuse the buffer created, since
        self.buffer = Some(buffer);

        if self.first_configure {
            self.first_configure = false;
        }
        self.frame_called = false;

        // self.decoder = Some(decoder);
        self.decoder = None;
        Ok(RenderStatus::VideoEnd)
    }

    fn img(&self) -> (PathBuf, String) {
        self.file_list[self.index].clone()
    }

    /// if there is an image on current_img, give the image and increase index
    fn next(&mut self) {
        let img_list = &self.file_list;
        let index = self.index;
        debug!("Current index is {}", index);
        if index < img_list.len() - 1 {
            self.index = index + 1;
        }
        if index == img_list.len() - 1 {
            self.index = 0;
        }

        debug!("new index is {}", self.index);
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
        // TODO: use scale factor value given
        debug!("{:?} - {}", surface, new_factor);
    }

    fn frame(
        &mut self,
        _conn: &client::Connection,
        _qh: &client::QueueHandle<Self>,
        surface: &client::protocol::wl_surface::WlSurface,
        time: u32,
    ) {
        // TODO: use frame somehow?
        debug!(
            "frame called for {:?} - {} from CompositorHandler",
            surface, time
        );
        let surface_id = surface.id().protocol_id();
        let output_name = self.layershell_to_outputname.get(&surface_id).unwrap();

        let output = self
            .outputs_map
            .get_mut(output_name)
            .expect("failed getting output_map");

        {
            let mut output = output.lock().unwrap();
            output.frame_called = true;
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

        let outputs = &mut self.outputs_map;
        match outputs.remove(&name) {
            Some(removed_output) => {
                info!("output {name} was removed");
                match self
                    .timer_tokens
                    .remove_entry(&removed_output.lock().unwrap().output_info.id)
                {
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
        debug!(
            "layershell id {} is closed",
            _layer.wl_surface().id().protocol_id()
        );
    }

    #[instrument(skip_all, fields(layer_id=layer.wl_surface().id().protocol_id(),_serial))]
    fn configure(
        &mut self,
        _conn: &client::Connection,
        _qh: &client::QueueHandle<Self>,
        layer: &smithay_client_toolkit::shell::wlr_layer::LayerSurface,
        configure: smithay_client_toolkit::shell::wlr_layer::LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let configure_surface_id = layer.wl_surface().id().protocol_id();
        let output_name = self
            .layershell_to_outputname
            .get(&configure_surface_id)
            .unwrap_or(&String::default())
            .to_owned();
        if let Some(output_original) = self.outputs_map.get_mut(&output_name) {
            // only if there is an output config do we render
            let mut output = output_original.lock().unwrap();
            if let Some(output_config) = output.output_config.clone() {
                if output.first_configure {
                    let output_id = output.output_info.id;
                    info!("first configure for surface {configure_surface_id}, output name {output_name}");
                    output.dimensions = Some(configure.new_size);

                    let (tx, rx) = std::sync::mpsc::channel();
                    let output_handle = output_original.clone();
                    let handle = std::thread::spawn(move || {
                        let output = output_handle;
                        let name = { output.lock().unwrap().output_name.clone() };
                        let mut cont = RenderStatus::Empty;
                        loop {
                            match cont {
                                RenderStatus::Video => {
                                    // TODO: fps regulation
                                    let mut output = output.lock().unwrap();
                                    cont = output.draw(cont, false).expect("next video frame");
                                }
                                RenderStatus::Empty
                                | RenderStatus::Image
                                | RenderStatus::VideoEnd => {
                                    match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                                        Ok(ev) => match ev {
                                            Msg::Draw => {
                                                info!("thread received draw command for output {name}");
                                                // TODO: log draw failure
                                                {
                                                    let mut output = output.lock().unwrap();
                                                    cont = output
                                                        .draw(cont, true)
                                                        .expect("draw success");
                                                }
                                            }
                                            Msg::Removed => break,
                                            _ => {}
                                        },
                                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                                        Err(_) => break,
                                    }
                                }
                                RenderStatus::Fail => {
                                    debug!("draw failed, looping");
                                    let mut output = output.lock().unwrap();
                                    cont = output.draw(cont, false).expect("hey");
                                }
                            }
                        }
                    });
                    tx.send(Msg::Draw).unwrap();

                    self.thread_map.insert(output_name.clone(), handle);

                    let timer = Timer::from_duration(std::time::Duration::from_secs(
                        output_config.duration.unwrap_or(60),
                    ));
                    let output_name_clone = output_name.clone();
                    let timer_token = self
                        .c_queue_handle
                        .insert_source(timer, move |deadline, _, data| {
                            trace!("timer reached deadline: {}", deadline.elapsed().as_secs());
                            if let Some(output) = data.outputs_map.get_mut(&output_name_clone) {
                                // TODO: log draw failure
                                let time = {
                                    output
                                        .lock()
                                        .unwrap()
                                        .output_config
                                        .as_ref()
                                        .unwrap()
                                        .duration
                                        .unwrap_or(60)
                                };
                                tx.send(Msg::Draw).expect("callback called");
                                return TimeoutAction::ToDuration(std::time::Duration::from_secs(
                                    time,
                                ));
                            }

                            return TimeoutAction::ToDuration(std::time::Duration::from_secs(60));
                        })
                        .expect("works");
                    self.timer_tokens.insert(output_id, timer_token);

                    // TODO: watch dir to update config
                } else if !output.first_configure && output.dimensions != Some(configure.new_size) {
                    warn!("received configure event, screen size changed");
                    output.dimensions = Some(configure.new_size);
                    output
                        .draw(RenderStatus::Empty, false)
                        .expect("success draw");
                } else if !output.first_configure && output.dimensions == Some(configure.new_size) {
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
