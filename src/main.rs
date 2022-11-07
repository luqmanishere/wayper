use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Instant,
};

use eyre::{eyre, Result};
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
    timer_token: HashMap<String, RegistrationToken>,
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
                Arc::new(Mutex::new(WallSurface::new(
                    &output,
                    info,
                    output_config,
                    surface,
                    &layer_shell.clone(),
                    pool,
                ))),
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

    // Set up wayland event loop
    let mut event_loop = calloop::EventLoop::<LoopState>::try_new().unwrap();

    WaylandSource::new(queue)
        .quick_insert(event_loop.handle())
        .unwrap();

    let _handle = Arc::clone(&surfaces);
    let handle = Arc::clone(&surfaces);
    //let mut update_handler = move |_, _, _| {};

    let mut timer_hashmap = HashMap::new();
    {
        for (_, surface) in handle.lock().unwrap().iter_mut() {
            let name = surface.lock().unwrap().output_info.name.clone();
            let surface = Arc::clone(surface);
            let calloop_timer = calloop::timer::Timer::immediate();
            let display_handle = display.clone();
            let timer_token = event_loop
                .handle()
                .insert_source(calloop_timer, move |deadline, _, _shared_data| {
                    debug!("calloop timer called for: {:?}", deadline);
                    let mut surface = surface.lock().unwrap();
                    if !surface.handle_events() {
                        let now = Instant::now();
                        if surface.should_redraw(&now) {
                            info!("surface will redraw");
                            match surface.draw(now) {
                                Ok(_) => {}
                                Err(e) => {
                                    error!("{e:?}");
                                }
                            };
                        }
                    }
                    display_handle.flush().unwrap();

                    // Set duration of next call
                    let duration = surface.output_config.duration.unwrap() as u64;
                    calloop::timer::TimeoutAction::ToDuration(std::time::Duration::from_secs(
                        duration,
                    ))
                })
                .unwrap();
            timer_hashmap.insert(name, timer_token);
        }
    }

    let (tx, rx): (Sender<()>, Channel<()>) = calloop::channel::channel();
    let watcher_config_handle = Arc::clone(&config);
    let watcher_surfaces_handle = Arc::clone(&surfaces);
    event_loop
        .handle()
        .insert_source(rx, move |_, _, _| {
            //info!("Callback called!");
            info!("Config changed!");
            let mut watcher_config_handle = watcher_config_handle.lock().unwrap();
            watcher_config_handle.update().unwrap();
            for (_, surface) in watcher_surfaces_handle.lock().unwrap().iter_mut() {
                let mut surface = surface.lock().unwrap();
                let new_config = watcher_config_handle
                    .get_output_config(&surface.output_info.name)
                    .unwrap();
                surface.update_config(new_config).unwrap();
                dbg!(&surface);
            }
        })
        .unwrap();

    // let (wtx, wrx) = std::sync::mpsc::channel();
    let mut debouncer = notify_debouncer_mini::new_debouncer(
        std::time::Duration::from_secs(10),
        None,
        move |res| match res {
            Ok(o) => {
                debug!("config watcher: {:?}", o);
                tx.send(()).unwrap();
            }
            Err(e) => {
                error!("config watcher: {:?}", e);
            }
        },
    )?;
    let watcher = debouncer.watcher();
    watcher.watch(config_path, notify::RecursiveMode::Recursive)?;
    let mut state = LoopState {
        handle: event_loop.handle(),
        timer_token: timer_hashmap,
    };
    event_loop.run(None, &mut state, |_shared_data| {})?;
    Ok(())
}
