use std::{collections::HashMap, ops::Deref, os::unix::net::UnixStream, path::Path};

use clap::{Parser, ValueEnum};
use color_eyre::Result;
use smithay_client_toolkit::{
    compositor::CompositorState,
    output::OutputState,
    reexports::{
        calloop::{self, EventLoop},
        calloop_wayland_source::WaylandSource,
        client::{Connection, globals::registry_queue_init},
    },
    registry::RegistryState,
    shell::wlr_layer::LayerShell,
    shm::Shm,
};
use tracing::{info, level_filters::LevelFilter};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{Layer as TLayer, fmt, prelude::__tracing_subscriber_SubscriberExt};
use wallpaperhandler::Wayper;
use wayper_lib::{
    config::WayperConfig,
    socket::{OutputWallpaper, SocketCommands, SocketError, SocketOutput, WayperSocket},
    utils::{
        map::{OutputKey, OutputMap},
        output::OutputRepr,
        render_server::RenderServer,
    },
};

mod wallpaperhandler;

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = WayperCli::parse();

    // logging setup
    let _guards = start_logging(cli.log_level);

    // config setup
    let config_path = if let Some(config_path) = cli.config {
        config_path
    } else {
        #[cfg(not(debug_assertions))]
        let config_path = Path::new("/home/luqman/.config/wayper/config.toml").into();

        #[cfg(debug_assertions)]
        let config_path = Path::new("./samples/test_config.toml").into();
        config_path
    };

    let config = WayperConfig::load(&config_path)?;

    // Get the wayland details from the env, initiate the wayland event source
    let conn = Connection::connect_to_env().expect("in a wayland session");
    let (globals, queue) = registry_queue_init(&conn).expect("event queue is initialized");
    let qh = queue.handle();
    let mut event_loop: EventLoop<Wayper> =
        EventLoop::try_new().expect("failed to initalize event_loop");
    let loop_handle = event_loop.handle();
    WaylandSource::new(conn.clone(), queue)
        .insert(loop_handle)
        .unwrap();

    // setup the wayland client and its handlers
    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor is not available");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("layer shell is not available");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm is not available");
    let output_state = OutputState::new(&globals, &qh);
    // let outputs_hashmap_arc: Arc<RwLock<HashMap<String, OutputRepr>>> = Default::default();
    let output_map: OutputMap = Default::default();

    // channel based event source for our socket
    let (socket_tx, socket_channel) = calloop::channel::channel::<UnixStream>();
    let mut socket = WayperSocket::new("/tmp/wayper/.socket.sock".into(), socket_tx);

    let outputs_map_handle = output_map.clone();
    // insert the channel receiver as a source in calloop
    event_loop
        .handle()
        .insert_source(socket_channel, move |ev, _, shared_data| {
            tracing::debug!("stream received from listener");
            match ev {
                calloop::channel::Event::Msg(stream) => {
                    shared_data.socket_counter += 1;
                    match handle_stream(
                        shared_data.socket_counter,
                        stream,
                        outputs_map_handle.clone(),
                    ) {
                        Ok(_) => {
                            tracing::debug!("stream is handled");
                        }
                        Err(e) => {
                            tracing::error!("error handling stream: {e}");
                        }
                    }
                }
                calloop::channel::Event::Closed => {
                    tracing::error!("socket input listener channel is closed!");
                }
            }
        })
        .unwrap();

    // TODO: remove this
    // keep this alive until the end of the program
    let _socket_listener = socket.socket_sender_thread();

    let mut data = wallpaperhandler::Wayper {
        compositor_state: compositor,
        registry_state: RegistryState::new(&globals),
        output_state,
        layer_shell,
        shm,
        outputs: output_map,
        config,
        c_queue_handle: event_loop.handle(),
        timer_tokens: HashMap::new(),
        socket_counter: 0,
        render_server: std::sync::Arc::new(RenderServer::new()),
    };

    loop {
        event_loop
            .dispatch(None, &mut data)
            .expect("event loop doesn't panic");
    }
}

#[tracing::instrument(skip_all, fields(counter = _counter))]
fn handle_stream(_counter: u64, mut stream: UnixStream, outputs: OutputMap) -> Result<()> {
    tracing::debug!("Socket call counter: {_counter}");
    let command = SocketCommands::from_socket(&mut stream)?;

    match command {
        SocketCommands::Ping => {
            SocketOutput::Message("pong".to_string()).write_to_socket(&mut stream)?;
        }
        SocketCommands::Current { output_name } => {
            /// Get the current image for the output and wrap it
            fn get_output_current_image(
                output: &OutputRepr,
            ) -> Result<OutputWallpaper, SocketError> {
                let output_name = &output.output_name;
                match output.current_img() {
                    Some(image_path) => Ok(OutputWallpaper {
                        output_name: output_name.to_string(),
                        wallpaper: image_path.display().to_string(),
                    }),
                    None => Err(SocketError::NoCurrentImage {
                        output: output_name.to_string(),
                    }),
                }
            }

            // if output name specified
            if let Some(output_name) = output_name {
                match outputs.get(OutputKey::OutputName(output_name.clone())) {
                    Some(output) => {
                        match get_output_current_image(output.lock().unwrap().deref()) {
                            Ok(outwp) => SocketOutput::CurrentWallpaper(outwp)
                                .write_to_socket(&mut stream)?,
                            Err(error) => error.write_to_socket(&mut stream)?,
                        }
                    }
                    None => SocketError::UnindentifiedOutput { output_name }
                        .write_to_socket(&mut stream)?,
                }
            } else {
                // i would use iterators here if i could, but i cant figure this out
                let mut output_wallpapers = vec![];
                let mut errors = vec![];
                for value in outputs.iter() {
                    let output = value.lock().unwrap();
                    match get_output_current_image(output.deref()) {
                        Ok(outwp) => output_wallpapers.push(outwp),
                        Err(error) => errors.push(error),
                    }
                }
                SocketOutput::Wallpapers(output_wallpapers).write_to_socket(&mut stream)?;
                // TODO: figure out how to send multi frame responses
                // SocketOutput::MultipleErrors(errors).write_to_socket(&mut stream)?;
            }
        }
        SocketCommands::Toggle { output_name } => {
            if let Some(output_name) = output_name {
                match outputs.get(OutputKey::OutputName(output_name.clone())) {
                    Some(output) => {
                        let mut output = output.lock().unwrap();
                        output.toggle_visible();
                        SocketOutput::Message(format!(
                            "Toggled visibility for output {}",
                            output.output_name
                        ))
                        .write_to_socket(&mut stream)?;
                    }
                    None => SocketError::UnindentifiedOutput { output_name }
                        .write_to_socket(&mut stream)?,
                }
            } else {
                let mut toggled = vec![];
                for value in outputs.iter() {
                    let mut output = value.lock().unwrap();
                    output.toggle_visible();
                    toggled.push(output.output_name.clone());
                }
                SocketOutput::Message(format!(
                    "Toggled visibility for outputs {}",
                    toggled.join(", ")
                ))
                .write_to_socket(&mut stream)?;
            }
        }
        // TODO: toggle
        // TODO: hide
        // TODO: show
        command => {
            SocketError::CommandUnimplemented {
                command: command.to_string(),
            }
            .write_to_socket(&mut stream)?;
        }
    }
    // else if msg == "toggle" {
    //     for (_, surface) in outputs.lock().unwrap().iter_mut() {
    //         surface.lock().unwrap().toggle_visiblity();
    //     }
    // } else if msg == "hide" {
    //     for (_, surface) in outputs.lock().unwrap().iter_mut() {
    //         surface.lock().unwrap().hide();
    //     }
    // } else if msg == "show" {
    //     for (_, surface) in outputs.lock().unwrap().iter_mut() {
    //         surface.lock().unwrap().show();
    //     }
    // }
    Ok(())
}

fn start_logging(file_log_level: LogLevel) -> Vec<WorkerGuard> {
    let mut guards = Vec::new();
    let file_appender = tracing_appender::rolling::never("/tmp/wayper", "wayper-log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    // tracing_appender::non_blocking::NonBlockingBuilder
    guards.push(guard);

    let subscriber = tracing_subscriber::registry()
        .with(
            fmt::Layer::new()
                .with_writer(non_blocking)
                .with_ansi(false)
                // .with_timer(tracing_subscriber::fmt::time::time())
                .with_filter(file_log_level.as_loglevel()),
        )
        .with(
            fmt::layer()
                .with_ansi(true)
                .with_timer(tracing_subscriber::fmt::time::time())
                .with_filter(file_log_level.as_loglevel()),
        );

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
    info!("logger started!");
    guards
}

#[derive(Parser)]
struct WayperCli {
    /// Path to the config to use
    #[arg(short, long)]
    config: Option<std::path::PathBuf>,

    /// Log level for file.
    #[arg(short, long, value_enum, default_value_t = LogLevel::Info)]
    log_level: LogLevel,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Default)]
enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}
impl LogLevel {
    pub fn as_loglevel(&self) -> tracing_subscriber::filter::LevelFilter {
        match self {
            LogLevel::Error => LevelFilter::ERROR,
            LogLevel::Warn => LevelFilter::WARN,
            LogLevel::Info => LevelFilter::INFO,
            LogLevel::Debug => LevelFilter::DEBUG,
            LogLevel::Trace => LevelFilter::TRACE,
        }
    }
}
