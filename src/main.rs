use std::{
    collections::HashMap,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    sync::{Arc, Mutex},
};

use eyre::{eyre, Context, Result};
use smithay_client_toolkit::{
    default_environment,
    environment::SimpleGlobal,
    new_default_environment,
    output::{with_output_info, OutputInfo},
    reexports::{
        calloop::{
            self,
            channel::{Channel, Sender},
            LoopHandle, RegistrationToken,
        },
        client::protocol::wl_output,
        protocols::wlr::unstable::layer_shell::v1::client::zwlr_layer_shell_v1,
    },
    WaylandSource,
};
use tracing::{debug, error, info};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{filter, fmt, prelude::__tracing_subscriber_SubscriberExt, Layer};

use crate::surface::WallSurface;

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

#[allow(dead_code)]
pub struct LoopState {
    handle: LoopHandle<'static, Self>,
    timer_token: Arc<Mutex<HashMap<String, RegistrationToken>>>,
    surfaces: Arc<Mutex<Vec<(u32, Arc<Mutex<WallSurface>>)>>>,
    socket_counter: u64,
}

fn start_logging() -> Vec<WorkerGuard> {
    let mut guards = Vec::new();
    let file_appender = tracing_appender::rolling::daily("/tmp/wayper", "log");
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
    info!("logger started!");
    guards
}

fn main() -> Result<()> {
    let _guards = start_logging();
    let (env, display, queue) =
        new_default_environment!(Env, fields = [layer_shell: SimpleGlobal::new(),])
            .expect("Initial roundtrip failed!");

    let config_path = Path::new("/home/luqman/.config/wayper/config.toml");
    let config = Arc::new(Mutex::new(crate::config::Config::load(config_path)?));

    let surfaces = Arc::new(Mutex::new(Vec::new()));

    let layer_shell = env.require_global::<zwlr_layer_shell_v1::ZwlrLayerShellV1>();

    // create calloop event loop
    let mut event_loop = calloop::EventLoop::<LoopState>::try_new().unwrap();
    WaylandSource::new(queue)
        .quick_insert(event_loop.handle())
        .unwrap();
    let timer_token_hashmap = Arc::new(Mutex::new(HashMap::new()));

    let env_handle = env.clone();
    let surfaces_handle = Arc::clone(&surfaces);
    let config_handle = Arc::clone(&config);
    let display_handle = display.clone();
    let event_loop_handle = event_loop.handle();
    let timer_token_hashmap_handle = timer_token_hashmap.clone();
    // TODO: use ids instead of name
    let output_handler = move |output: wl_output::WlOutput, info: &OutputInfo| {
        if info.obsolete {
            // delete surface
            surfaces_handle
                .lock()
                .unwrap()
                .retain(|(i, _)| *i != info.id);

            //  release the output
            output.release();

            // delete the timer associated with the output
            let mut timer_token_hashmap = { timer_token_hashmap_handle.lock().unwrap() };
            let token = timer_token_hashmap.remove(&info.name).unwrap();
            event_loop_handle.remove(token);
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
            let wall_surface = Arc::new(Mutex::new(WallSurface::new(
                &output,
                info,
                output_config,
                surface,
                &layer_shell.clone(),
                pool,
            )));

            let name = info.name.clone();
            let calloop_timer = calloop::timer::Timer::immediate();
            let display_handle = display_handle.clone();
            let surface_handle = wall_surface.clone();
            // insert timer and store its token
            let timer_token = event_loop_handle
                .insert_source(calloop_timer, move |deadline, _, _shared_data| {
                    debug!("calloop timer called for: {:?}", deadline);
                    let mut surface = surface_handle.lock().unwrap();
                    if !surface.handle_events() {
                        info!("surface will redraw");
                        match surface.draw() {
                            Ok(_) => {}
                            Err(e) => {
                                error!("{e:?}");
                            }
                        };
                    }
                    display_handle.flush().unwrap();

                    // Set duration of next call
                    let duration = surface.output_config.duration.unwrap() as u64;
                    calloop::timer::TimeoutAction::ToDuration(std::time::Duration::from_secs(
                        duration,
                    ))
                })
                .unwrap();

            // store timer tokens
            (*timer_token_hashmap_handle.lock().unwrap()).insert(name, timer_token);
            // store handle to surfaces
            (*surfaces_handle.lock().unwrap()).push((info.id, wall_surface));
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

    let (config_watcher_tx, config_watcher_channel): (Sender<()>, Channel<()>) =
        calloop::channel::channel();
    let config_watcher_config_handle = Arc::clone(&config);
    let config_watcher_surfaces_handle = Arc::clone(&surfaces);
    event_loop
        .handle()
        .insert_source(config_watcher_channel, move |_, _, _shared_data| {
            info!("Config changed!");
            let mut config_watcher_config_handle = config_watcher_config_handle.lock().unwrap();
            config_watcher_config_handle.update().unwrap();
            for (_, surface) in config_watcher_surfaces_handle.lock().unwrap().iter_mut() {
                let mut surface = surface.lock().unwrap();
                let new_config = config_watcher_config_handle
                    .get_output_config(&surface.output_info.name)
                    .unwrap();
                surface.update_config(new_config).unwrap();
                dbg!(&surface);
            }
        })
        .unwrap();

    let mut debouncer = notify_debouncer_mini::new_debouncer(
        std::time::Duration::from_secs(10),
        None,
        move |res| match res {
            Ok(o) => {
                debug!("config watcher: {:?}", o);
                config_watcher_tx.send(()).unwrap();
            }
            Err(e) => {
                error!("config watcher: {:?}", e);
            }
        },
    )?;
    let watcher = debouncer.watcher();
    watcher.watch(config_path, notify::RecursiveMode::Recursive)?;
    let watcher_surfaces_handle = Arc::clone(&surfaces);
    {
        for (_, surface) in watcher_surfaces_handle.lock().unwrap().iter() {
            let surface = surface.lock().unwrap();
            watcher.watch(
                surface.output_config.path.clone().unwrap().as_path(),
                notify::RecursiveMode::Recursive,
            )?;
        }
    }

    let (socket_tx, socket_channel): (Sender<UnixStream>, Channel<UnixStream>) =
        calloop::channel::channel::<UnixStream>();
    event_loop
        .handle()
        .insert_source(socket_channel, move |ev, _, shared_data| {
            debug!("stream received from listener");
            match ev {
                calloop::channel::Event::Msg(eve) => {
                    shared_data.socket_counter += 1;
                    match handle_stream(
                        shared_data.socket_counter,
                        eve,
                        Arc::clone(&shared_data.surfaces),
                    ) {
                        Ok(_) => {
                            debug!("stream is handled");
                        }
                        Err(e) => {
                            error!("error handling stream: {e}");
                        }
                    }
                }
                calloop::channel::Event::Closed => {}
            }
            //
        })
        .unwrap();
    std::thread::spawn(move || -> Result<()> {
        let socket_path = "/tmp/wayper/.socket.sock";
        if std::fs::metadata(socket_path).is_ok() {
            info!("previous socket detected");
            info!("removing previous socket");
            std::fs::remove_file(socket_path)
                .with_context(|| eyre!("could not delete previous socket at {:?}", socket_path))?;
        }
        let unix_listener = UnixListener::bind(socket_path)
            .with_context(|| eyre!("could not create unix socket"))?;

        loop {
            match unix_listener.accept() {
                Ok((unix_stream, _)) => socket_tx.send(unix_stream).unwrap(),
                Err(e) => {
                    error!("failed accepting connection from unixlistener: {e}");
                    continue;
                }
            }
        }
    });

    let mut state = LoopState {
        handle: event_loop.handle(),
        timer_token: timer_token_hashmap,
        surfaces: Arc::clone(&surfaces),
        socket_counter: 0,
    };
    event_loop.run(None, &mut state, |_shared_data| {})?;

    Ok(())
}

#[tracing::instrument(skip_all, fields(counter = _counter))]
fn handle_stream(
    _counter: u64,
    mut stream: UnixStream,
    surfaces: Arc<Mutex<Vec<(u32, Arc<Mutex<WallSurface>>)>>>,
) -> Result<()> {
    let mut msg = String::new();
    stream
        .read_to_string(&mut msg)
        .context("failed to read the stream")?;
    debug!("msg received on socket1: {msg}");

    if msg == "ping" {
        write_to_stream(&mut stream, "pong".to_string())?;
    } else if msg == "current" {
        for (_, surface) in surfaces.lock().unwrap().iter() {
            let surface = surface.lock().unwrap();
            let surface_name = surface.output_info.name.clone();
            let wallpaper = surface.current_img();
            write_to_stream(&mut stream, surface_name)?;
            let wallpaper = format!("{}\n", wallpaper.display());
            write_to_stream(&mut stream, wallpaper)?;
        }
    } else if msg == "toggle" {
        for (_, surface) in surfaces.lock().unwrap().iter_mut() {
            surface.lock().unwrap().toggle_visiblity();
        }
    } else if msg == "hide" {
        for (_, surface) in surfaces.lock().unwrap().iter_mut() {
            surface.lock().unwrap().hide();
        }
    } else if msg == "show" {
        for (_, surface) in surfaces.lock().unwrap().iter_mut() {
            surface.lock().unwrap().show();
        }
    } else {
        write_to_stream(&mut stream, "not implemented".to_string())?;
    }
    Ok(())
}

/// Helper function to write to a unix stream
fn write_to_stream(stream: &mut UnixStream, mut s: String) -> Result<()> {
    if !s.ends_with('\n') {
        s.push('\n');
    }
    stream
        .write(s.as_bytes())
        .wrap_err("failed to write to stream")?;
    debug!("wrote to stream: {}", s.trim());
    Ok(())
}
