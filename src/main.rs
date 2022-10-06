use std::{
    cell::Cell,
    convert::TryInto,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
    time::Instant,
};

use config::OutputConfig;
use derivative::Derivative;
use eyre::{eyre, Context, Result};
use image::imageops::FilterType;
use notify::Watcher;
use rand::seq::SliceRandom;
use smithay_client_toolkit::{
    default_environment,
    environment::SimpleGlobal,
    new_default_environment,
    output::{with_output_info, OutputInfo},
    reexports::{
        calloop::{
            self,
            channel::{Channel, Sender},
        },
        client::{
            protocol::{wl_buffer, wl_output, wl_shm, wl_surface},
            Attached, Main,
        },
        protocols::wlr::unstable::layer_shell::v1::client::{
            zwlr_layer_shell_v1, zwlr_layer_surface_v1,
        },
    },
    shm::AutoMemPool,
    WaylandSource,
};
use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::{filter, fmt, prelude::__tracing_subscriber_SubscriberExt, Layer};
use walkdir::WalkDir;

mod config;
mod surface;

/*
struct Env {
    compositor: SimpleGlobal<WlCompositor>,
    outputs: OutputHandler,
    shm: ShmHandler,
    xdg_output: XdgOutputHandler,
    layer_shell: SimpleGlobal<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
}

environment!(Env,
    singles = [
        WlCompositor => compositor,
        WlShm => shm,
        zwlr_layer_shell_v1::ZwlrLayerShellV1 => layer_shell,
        zxdg_output_manager_v1::ZxdgOutputManagerV1 => xdg_output,
    ],
    multis = [
        WlOutput => outputs,
    ]
);
        */

// FIXME: properly use eyre and remove unwraps
default_environment!(Env,
    fields = [
        layer_shell: SimpleGlobal<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    ],
    singles = [
        zwlr_layer_shell_v1::ZwlrLayerShellV1 => layer_shell
    ],
);

#[derive(PartialEq, Clone, Copy)]
enum RenderEvent {
    Configure { width: u32, height: u32 },
    Closed,
}

#[derive(Derivative)]
#[derivative(Debug)]
pub struct WallSurface {
    surface: wl_surface::WlSurface,
    layer_surface: Main<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    #[derivative(Debug = "ignore")]
    next_render_event: Rc<Cell<Option<RenderEvent>>>,
    pool: AutoMemPool,
    dimensions: (u32, u32),
    current_img: Option<PathBuf>,
    buffer: Option<wl_buffer::WlBuffer>,
    #[derivative(Debug = "ignore")]
    guard: Option<timer::Guard>,
    time_passed: Instant,
    redraw: bool,
    output_info: OutputInfo,
    output_config: Arc<OutputConfig>,
    img_list: Option<Vec<PathBuf>>,
    index: Option<usize>,
}

impl WallSurface {
    fn new(
        output: &wl_output::WlOutput,
        output_info: &OutputInfo,
        output_config: Arc<OutputConfig>,
        surface: wl_surface::WlSurface,
        layer_shell: &Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
        pool: AutoMemPool,
    ) -> Self {
        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            Some(output),
            zwlr_layer_shell_v1::Layer::Overlay,
            "wayper".to_owned(),
        );
        // set size to 0 so the compositor gives us the size
        layer_surface.set_size(0, 0);
        // input devices do not interact with us
        layer_surface
            .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
        // ignore exclusive zones and render on the entire screen
        layer_surface.set_exclusive_zone(-1);
        // anchor on all sides to cover the entire screen
        layer_surface.set_anchor(zwlr_layer_surface_v1::Anchor::all());
        // use the `background` layer, which puts us on the back
        layer_surface.set_layer(zwlr_layer_shell_v1::Layer::Background);

        // handle events
        let next_render_event = Rc::new(Cell::new(None::<RenderEvent>));
        let next_render_event_handle = Rc::clone(&next_render_event);
        layer_surface.quick_assign(move |layer_surface, event, _| {
            match (event, next_render_event_handle.get()) {
                (zwlr_layer_surface_v1::Event::Closed, _) => {
                    next_render_event_handle.set(Some(RenderEvent::Closed));
                }
                (
                    zwlr_layer_surface_v1::Event::Configure {
                        serial,
                        width,
                        height,
                    },
                    next,
                ) if next != Some(RenderEvent::Closed) => {
                    layer_surface.ack_configure(serial);
                    next_render_event_handle.set(Some(RenderEvent::Configure { width, height }));
                }
                (_, _) => {}
            }
        });

        // Commit so the server sends a configure event
        surface.commit();

        let img_list = if output_config.path.as_ref().unwrap().is_dir() {
            let mut files = WalkDir::new(&output_config.path.as_ref().unwrap())
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
            dbg!(&files);
            Some(files)
        } else {
            None
        };

        let current_img = if output_config.path.as_ref().unwrap().is_file() {
            let path = &output_config.path;
            if mime_guess::from_path(&path.as_ref().unwrap())
                .iter()
                .any(|ev| ev.type_() == "image")
            {
                path.clone()
            } else {
                None
            }
        } else {
            None
        };

        Self {
            surface,
            output_info: output_info.clone(),
            output_config,
            layer_surface,
            next_render_event,
            pool,
            dimensions: (0, 0),
            current_img,
            buffer: None,
            guard: None,
            time_passed: Instant::now(),
            redraw: true,
            img_list,
            index: None,
        }
    }

    /// Handle events that have occured since the last call
    /// returns true if the surface should be dropped
    fn handle_events(&mut self) -> bool {
        match self.next_render_event.take() {
            Some(RenderEvent::Closed) => true,
            Some(RenderEvent::Configure { width, height }) => {
                if self.dimensions != (width, height) {
                    self.dimensions = (width, height);
                }
                false
            }
            None => false,
        }
    }

    fn should_redraw(&mut self, time: &Instant) -> bool {
        let elapsed = {
            let delta = time.duration_since(self.time_passed);
            trace!("time since last update: {:?}", delta);
            std::time::Duration::from_secs(self.output_config.duration.unwrap() as u64 * 60)
                .saturating_sub(delta)
                == std::time::Duration::ZERO
        };

        (self.redraw || elapsed) && self.dimensions.0 != 0
    }

    fn next(&mut self) -> Option<&PathBuf> {
        if let Some(img_list) = &self.img_list {
            if let Some(index) = self.index {
                debug!("Current index is {}", index);
                if index < img_list.len() - 1 {
                    self.index = Some(index + 1);
                }
                if index == img_list.len() - 1 {
                    self.index = Some(0);
                }

                debug!("new index is {}", self.index.unwrap());
            } else {
                self.index = Some(0);
            }
            img_list.get(self.index.unwrap())
        } else {
            None
        }
    }

    /// the drawing function
    #[tracing::instrument(skip_all, fields(output = %self.output_info.name))]
    fn draw(&mut self, now: Instant) -> Result<()> {
        if self.dimensions == (0, 0) {
            return Err(eyre!(
                "dimensions are 0! has compositor sent a configure event?"
            ));
        }
        let stride = 4 * self.dimensions.0 as i32;
        let width = self.dimensions.0 as i32;
        let height = self.dimensions.1 as i32;

        let path = if let Some(path) = self.next() {
            path.clone()
        } else {
            self.current_img
                .as_ref()
                .ok_or_else(|| eyre!("current_img is empty!"))?
                .to_path_buf()
        };

        let image = image::open(&path)?
            .resize_to_fill(
                width.try_into().unwrap(),
                height.try_into().unwrap(),
                FilterType::Lanczos3,
            )
            .into_rgba8();

        // Destroy current buffer if we have one
        if let Some(buffer) = &self.buffer {
            buffer.destroy();
        }

        // create new buffer, draw image to buffer
        self.buffer = Some(
            self.pool
                .try_draw::<_, eyre::Error>(
                    width,
                    height,
                    stride,
                    wl_shm::Format::Abgr8888,
                    |canvas: &mut [u8]| {
                        let mut writer = BufWriter::new(canvas);
                        writer.write_all(image.as_raw()).unwrap();
                        writer.flush().unwrap();
                        Ok(())
                    },
                )
                .context("creating wl_buffer in pool")?,
        );

        // Attach the buffer to the surface and mark the entire surface as damaged
        self.surface.attach(self.buffer.as_ref(), 0, 0);
        self.surface
            .damage_buffer(0, 0, width as i32, height as i32);

        // commit
        self.surface.commit();
        self.redraw = false;
        self.time_passed = now;
        info!("Finished drawing current wallpaper: {}", path.display());
        Ok(())
    }

    fn update_config(&mut self, output_config: Arc<OutputConfig>) -> Result<()> {
        debug!("Updating config");
        self.img_list = if output_config.path.as_ref().unwrap().is_dir() {
            debug!("Detected path as a directory");
            let mut files = WalkDir::new(&output_config.path.as_ref().unwrap())
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
            dbg!(&files);
            Some(files)
        } else {
            None
        };

        self.current_img = if output_config.path.as_ref().unwrap().is_file() {
            debug!("Detected path as a file");
            let path = &output_config.path;
            if mime_guess::from_path(&path.as_ref().unwrap())
                .iter()
                .any(|ev| ev.type_() == "image")
            {
                path.clone()
            } else {
                None
            }
        } else {
            None
        };
        self.output_config = output_config;
        self.redraw = true;
        Ok(())
    }
}

impl Drop for WallSurface {
    fn drop(&mut self) {
        self.layer_surface.destroy();
        self.surface.destroy();
    }
}

fn main() -> Result<()> {
    let mut guards = Vec::new();

    let file_appender = tracing_appender::rolling::hourly("/tmp", "wayper");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    guards.push(guard);

    let subscriber = tracing_subscriber::registry()
        .with(
            fmt::Layer::new()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_timer(tracing_subscriber::fmt::time::time())
                .with_filter(
                    filter::EnvFilter::builder()
                        .with_default_directive(filter::LevelFilter::INFO.into())
                        .from_env_lossy(),
                ),
        )
        .with(
            fmt::layer()
                .with_ansi(true)
                .with_timer(tracing_subscriber::fmt::time::time())
                .with_filter(filter::LevelFilter::INFO),
        );

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
    let (env, display, queue) =
        new_default_environment!(Env, fields = [layer_shell: SimpleGlobal::new(),])
            .expect("Initial roundtrip failed!");

    let config_path = Path::new("/home/luqman/.config/wayper/config.toml");
    let config = Arc::new(Mutex::new(crate::config::Config::load(config_path)?));

    let surfaces = Arc::new(Mutex::new(Vec::new()));

    let layer_shell = env.require_global::<zwlr_layer_shell_v1::ZwlrLayerShellV1>();

    let env_handle = env.clone();
    let surfaces_handle = Arc::clone(&surfaces);
    let config_handle = Arc::clone(&config);
    let output_handler = move |output: wl_output::WlOutput, info: &OutputInfo| {
        if info.obsolete {
            // output removed, release the output
            surfaces_handle
                .lock()
                .unwrap()
                .retain(|(i, _)| *i != info.id);
            output.release();
        } else {
            dbg!(info);
            // output created, make a new surface for it
            let surface = env_handle.create_surface().detach();
            let pool = env_handle
                .create_auto_pool()
                .expect("Failed to create a memory pool!");
            let output_config = {
                let config_handle = config_handle.lock().unwrap();
                Arc::clone(
                    config_handle
                        .outputs
                        .get(&info.name)
                        .ok_or_else(|| eyre!("Can't find config for that output"))
                        .unwrap(),
                )
            };
            (*surfaces_handle.lock().unwrap()).push((
                info.id,
                WallSurface::new(
                    &output,
                    info,
                    output_config,
                    surface,
                    &layer_shell.clone(),
                    pool,
                ),
            ));
        }
    };

    // process existing outputs
    for output in env.get_all_outputs() {
        if let Some(info) = with_output_info(&output, Clone::clone) {
            output_handler(output, &info);
        }
    }

    // Setup a listener for output changes
    // the listener will live for as long as we keep the handle alive
    let _listener_handle =
        env.listen_for_outputs(move |output, info, _| output_handler(output, info));

    let mut event_loop = calloop::EventLoop::<()>::try_new().unwrap();

    let timer = timer::Timer::new();

    WaylandSource::new(queue)
        .quick_insert(event_loop.handle())
        .unwrap();

    let (tx, rx): (Sender<()>, Channel<()>) = calloop::channel::channel();
    event_loop
        .handle()
        .insert_source(rx, |_, _, _| {
            //info!("Callback called!");
        })
        .unwrap();

    let (wtx, wrx) = std::sync::mpsc::channel();
    let mut debouncer = notify_debouncer_mini::new_debouncer(
        std::time::Duration::from_secs(10),
        None,
        move |res| match res {
            Ok(o) => {
                debug!("config watcher: {:?}", o);
                wtx.send(()).unwrap();
            }
            Err(e) => {
                error!("config watcher: {:?}", e);
            }
        },
    )?;
    let watcher = debouncer.watcher();
    watcher.watch(config_path, notify::RecursiveMode::Recursive)?;

    loop {
        // TODO: Have better looping logic lmao why is the timer not working please look intocalloop

        // Check for config updates
        match wrx.try_recv() {
            Ok(_) => {
                info!("Config changed!");
                let config_handle = Arc::clone(&config);
                let mut config_handle = config_handle.lock().unwrap();
                config_handle.update()?;
                let handle = Arc::clone(&surfaces);
                for (_, surface) in handle.lock().unwrap().iter_mut() {
                    let new_config = config_handle.get_output_config(&surface.output_info.name)?;
                    surface.update_config(new_config)?;
                    dbg!(&surface);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
        }

        {
            let handle = Arc::clone(&surfaces);
            for (_, surface) in handle.lock().unwrap().iter_mut() {
                poll(tx.clone(), &timer, surface);
            }
        }
        display.flush().unwrap();
        // dispatch event
        event_loop.dispatch(None, &mut ()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10))
    }
}

/// Poll the surfaces' timers, redraw if timer is over
fn poll(tx: Sender<()>, timer: &timer::Timer, surface: &mut WallSurface) {
    if !surface.handle_events() {
        let now = Instant::now();
        if surface.should_redraw(&now) {
            info!("Surface will redraw");
            match surface.draw(now) {
                Ok(_) => {
                    surface.guard = Some(timer.schedule_with_delay(
                        chrono::Duration::minutes(surface.output_config.duration.unwrap() as i64),
                        move || {
                            tx.send(()).unwrap();
                        },
                    ));
                }
                Err(e) => {
                    error!("{e:?}");
                    //tx.send(()).unwrap();
                }
            };
        }
    }
}
