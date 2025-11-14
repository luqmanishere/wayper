use std::{collections::HashMap, ops::Deref, path::Path, sync::mpsc::SyncSender, time::Instant};

use clap::{Parser, ValueEnum};
use color_eyre::Result;
use smithay_client_toolkit::{
    compositor::CompositorState,
    output::OutputState,
    reexports::{
        calloop::{self, EventLoop, channel::Event},
        calloop_wayland_source::WaylandSource,
        client::{Connection, globals::registry_queue_init},
    },
    registry::RegistryState,
    shell::wlr_layer::LayerShell,
    shm::Shm,
};
use tracing::{info, level_filters::LevelFilter};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_log::LogTracer;
use tracing_subscriber::{Layer as TLayer, fmt, prelude::__tracing_subscriber_SubscriberExt};
use handlers::Wayper;
use wayper_lib::{
    config::Config,
    socket::{OutputWallpaper, SocketCommand, SocketError, SocketOutput, WayperSocket, get_socket_path},
};

use crate::{
    map::{OutputKey, OutputMap},
    output::OutputRepr,
    wgpu_renderer::WgpuRenderer,
};

mod map;
mod metered_cache;
mod output;
// mod render_server;
mod handlers;
mod wgpu_renderer;

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = WayperCli::parse();

    // logging setup
    let _guards = start_logging(cli.log_level);

    // config setup
    let config_path = if let Some(config_path) = cli.config {
        config_path
    } else if !cfg!(debug_assertions) {
        Path::new("/home/luqman/.config/wayper/config.toml").into()
    } else {
        Path::new("./samples/test_config.toml").into()
    };

    let config = Config::load_file(&config_path)?;

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
    let (socket_tx, socket_channel) =
        calloop::channel::channel::<(SocketCommand, SyncSender<SocketOutput>)>();

    // Create a unique socket path per Wayland display
    let socket_path = get_socket_path()?;
    let mut socket = WayperSocket::new(socket_path, socket_tx);

    // insert the channel receiver as a source in calloop
    event_loop
        .handle()
        .insert_source(socket_channel, move |ev, _, shared_data| {
            tracing::debug!("stream received from listener");
            match ev {
                Event::Msg((socket_command, stream)) => {
                    shared_data.socket_counter += 1;

                    match handle_command(
                        shared_data.socket_counter,
                        socket_command,
                        stream,
                        shared_data,
                    ) {
                        Ok(_) => {
                            tracing::debug!("command is handled");
                        }
                        Err(e) => {
                            tracing::error!("error handling command: {e}");
                        }
                    }
                }
                Event::Closed => {
                    tracing::error!("socket input listener channel is closed!");
                }
            }
        })
        .unwrap();

    // keep this alive until the end of the program
    // This will return an error if another instance is already running
    socket.socket_sender_thread().map_err(|e| {
        tracing::error!("Failed to start socket listener: {}", e);
        color_eyre::eyre::eyre!(e.to_string())
    })?;

    let mut data = handlers::Wayper {
        compositor_state: compositor,
        registry_state: RegistryState::new(&globals),
        output_state,
        layer_shell,
        shm,
        current_profile: config.default_profile.clone(),
        outputs: output_map,
        config,
        c_queue_handle: event_loop.handle(),
        draw_tokens: HashMap::new(),
        socket_counter: 0,
        wgpu: WgpuRenderer::new(),
    };

    loop {
        event_loop
            .dispatch(None, &mut data)
            .expect("event loop doesn't panic");
    }
    // TODO: signals handler
    // drop(data.wgpu);
}

#[tracing::instrument(skip_all, fields(counter = _counter))]
fn handle_command(
    _counter: u64,
    socket_command: SocketCommand,
    reply_tx: SyncSender<SocketOutput>,
    wayper: &mut Wayper,
) -> Result<()> {
    tracing::debug!("Socket call counter: {_counter}");
    let mut socket_responses = vec![];
    let outputs = &wayper.outputs;
    let command_name = socket_command.to_string();

    match socket_command {
        SocketCommand::Ping => socket_responses.push(SocketOutput::Message("pong".to_string())),
        SocketCommand::Current { output_name } => {
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
                            Ok(outwp) => {
                                socket_responses.push(SocketOutput::CurrentWallpaper(outwp))
                            }
                            Err(error) => socket_responses.push(error.into()),
                        }
                    }
                    None => socket_responses
                        .push(SocketError::UnindentifiedOutput { output_name }.into()),
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
                socket_responses.push(SocketOutput::Wallpapers(output_wallpapers));
                // TODO: figure out how to send multi frame responses
                socket_responses.push(SocketOutput::MultipleErrors(errors));
            }
        }
        SocketCommand::Toggle { output_name } => {
            if let Some(output_name) = output_name {
                match outputs.get(OutputKey::OutputName(output_name.clone())) {
                    Some(output) => {
                        let mut output = output.lock().unwrap();
                        output.toggle_visible();
                        socket_responses.push(SocketOutput::Message(format!(
                            "Toggled visibility for output {}",
                            output.output_name
                        )));
                    }
                    None => socket_responses
                        .push(SocketError::UnindentifiedOutput { output_name }.into()),
                }
            } else {
                let mut toggled = vec![];
                for value in outputs.iter() {
                    let mut output = value.lock().unwrap();
                    output.toggle_visible();
                    toggled.push(output.output_name.clone());
                }
                socket_responses.push(SocketOutput::Message(format!(
                    "Toggled visibility for outputs {}",
                    toggled.join(", ")
                )));
            }
        }
        SocketCommand::ChangeProfile { profile_name } => {
            match wayper.change_profile(profile_name.clone()) {
                Ok(profile_name) => socket_responses.push(SocketOutput::Message(format!(
                    "Changed profile to: {profile_name}"
                ))),
                Err(_err) => {
                    socket_responses.push(SocketOutput::SingleError(SocketError::NoProfile(
                        profile_name.unwrap_or(wayper.config.default_profile.clone()),
                    )))
                }
            };
        }
        SocketCommand::Profiles => {
            socket_responses.push(SocketOutput::Profiles(wayper.config.profiles.profiles()))
        }
        SocketCommand::GpuMetrics => {
            let metrics = wayper.wgpu.get_metrics_data();
            socket_responses.push(SocketOutput::GpuMetrics(metrics));
        }
        // TODO: hide
        // TODO: show
        command => socket_responses.push(
            SocketError::CommandUnimplemented {
                command: command.to_string(),
            }
            .into(),
        ),
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

    socket_responses.push(SocketOutput::End(command_name));
    for response in socket_responses {
        reply_tx.send(response)?;
    }

    Ok(())
}

/// Custom timer that displays both wall-clock time and uptime since application start
#[derive(Clone)]
struct UptimeTimer {
    start: Instant,
}

impl UptimeTimer {
    fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl tracing_subscriber::fmt::time::FormatTime for UptimeTimer {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        // Format wall-clock time
        let now = std::time::SystemTime::now();
        let datetime: chrono::DateTime<chrono::Utc> = now.into();
        write!(w, "{}", datetime.format("%Y-%m-%dT%H:%M:%S%.3fZ"))?;

        // Format uptime
        let elapsed = self.start.elapsed();
        let secs = elapsed.as_secs_f64();
        write!(w, " [+{:.3}s]", secs)?;

        Ok(())
    }
}

fn start_logging(file_log_level: LogLevel) -> Vec<WorkerGuard> {
    let mut guards = Vec::new();
    let file_appender = tracing_appender::rolling::never("/tmp/wayper", "wayper-log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    // tracing_appender::non_blocking::NonBlockingBuilder
    guards.push(guard);

    let uptime_timer = UptimeTimer::new();

    let subscriber = tracing_subscriber::registry()
        .with(
            fmt::Layer::new()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_timer(uptime_timer.clone())
                .with_filter(file_log_level.as_loglevel()),
        )
        .with(
            fmt::layer()
                .with_ansi(true)
                .with_timer(uptime_timer)
                .with_filter(file_log_level.as_loglevel()),
        );

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
    LogTracer::init().expect("logger facade initialized");
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
