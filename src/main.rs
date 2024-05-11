use std::{collections::HashMap, path::Path, time::Duration};

use eyre::Result;
use smithay_client_toolkit::{
    compositor::CompositorState,
    output::OutputState,
    reexports::{
        calloop::EventLoop,
        calloop_wayland_source::WaylandSource,
        client::{globals::registry_queue_init, Connection},
    },
    registry::RegistryState,
    shell::wlr_layer::LayerShell,
    shm::Shm,
};
use tracing::info;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    filter, fmt, prelude::__tracing_subscriber_SubscriberExt, Layer as TLayer,
};
use wallpaperhandler::Wayper;

mod config;
// mod surface;
mod wallpaperhandler;

fn start_logging() -> Vec<WorkerGuard> {
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
                .with_filter(
                    filter::EnvFilter::builder()
                        .with_env_var("RUST_LOG")
                        .with_default_directive(filter::LevelFilter::DEBUG.into())
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
    // logging setup
    let _guards = start_logging();
    video_rs::init().unwrap();
    // config setup
    #[cfg(not(debug_assertions))]
    let config_path = Path::new("/home/luqman/.config/wayper/config.toml");

    #[cfg(debug_assertions)]
    let config_path = Path::new("./samples/test_config.toml");
    let config = crate::config::WayperConfig::load(config_path)?;

    // wayland env connection
    let conn = Connection::connect_to_env().expect("in a wayland session");
    let (globals, queue) = registry_queue_init(&conn).expect("event queue is initialized");
    let qh = queue.handle();
    let mut event_loop: EventLoop<Wayper> =
        EventLoop::try_new().expect("failed to initalize event_loop");
    let loop_handle = event_loop.handle();
    WaylandSource::new(conn.clone(), queue)
        .insert(loop_handle)
        .unwrap();

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor is not available");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("layer shell is not available");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm is not available");
    let output_state = OutputState::new(&globals, &qh);

    let mut data = wallpaperhandler::Wayper {
        compositor_state: compositor,
        registry_state: RegistryState::new(&globals),
        output_state,
        layer_shell,
        shm,
        outputs_map: HashMap::new(),
        layershell_to_outputname: HashMap::new(),
        config,
        c_queue_handle: event_loop.handle(),
        thread_map: HashMap::new(),
        timer_tokens: HashMap::new(),
    };

    loop {
        event_loop
            .dispatch(Duration::from_millis(30), &mut data)
            .expect("no breaking");
    }
    /*
    let timer_token_hashmap = Arc::new(Mutex::new(HashMap::new()));

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

        */
}

/*
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
*/
