use std::{path::PathBuf, sync::Arc, time::Instant};

use clap::Parser;
use env_logger::Env;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use wayper_windows::config::{Config, ResolvedContent, default_config_path};
use wayper_windows::windows_host::{
    find_shelldll_defview, get_progman, reparent_window, set_z_pos, spawn_workerw,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetParent, GetWindowLongPtrW,
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ControlFlow, EventLoop, EventLoopProxy},
    window::Window,
};

use crate::engine::Engine;

mod engine;
mod player;
mod renderer;

fn main() -> color_eyre::Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or(if cfg!(debug_assertions) {
        "debug"
    } else {
        "info"
    }))
    .init();
    color_eyre::install()?;

    log::info!("logger initialized!");

    let args = Args::parse();
    let config_path = args.config.unwrap_or_else(default_config_path);
    let config = Config::load_file(&config_path)?;
    let mut app = App::new(config)?;
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    install_ctrl_c_handler(proxy);
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    config: Option<PathBuf>,
}

struct App {
    engine: Engine,
}

impl App {
    pub fn new(config: Config) -> color_eyre::Result<Self> {
        let resolved_image = match config.resolve_content()? {
            ResolvedContent::Image(image) => image,
            ResolvedContent::Video(_) => {
                color_eyre::eyre::bail!("video content is not implemented yet in wayper-windows")
            }
            ResolvedContent::Scene(_) => {
                color_eyre::eyre::bail!("scene content is not implemented yet in wayper-windows")
            }
        };

        Ok(Self {
            engine: pollster::block_on(Engine::new(resolved_image))?,
        })
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        log::debug!("resumed called, checking for new windows to spawn");

        for mon in event_loop.available_monitors() {
            let output_iden = mon.name().unwrap_or("Unknown".to_string());
            if !self.engine.has_output(&output_iden) {
                // make new window
                let size = mon.size();
                let window = Arc::new(
                    event_loop
                        .create_window(
                            Window::default_attributes()
                                .with_resizable(false)
                                .with_decorations(false),
                        )
                        .expect("Can create windows"),
                );
                log::info!("window {:?} created", window.id());

                if let Err(err) = self
                    .engine
                    .add_output(output_iden.clone(), window.clone(), size)
                {
                    log::error!("failed to add output {output_iden}: {err}");
                    event_loop.exit();
                    return;
                }

                // windows stuff
                let progman = get_progman().unwrap();
                let workerw = spawn_workerw(progman).unwrap();
                let shelldll = find_shelldll_defview(progman).unwrap();
                log_window_state("main pre-attach", hwnd_from_window(&window).unwrap());
                reparent_window(progman, window.clone()).unwrap();
                set_z_pos(shelldll, workerw, window.clone()).unwrap();
                log_window_state("main post-attach", hwnd_from_window(&window).unwrap());

                window.request_redraw();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        log::trace!("Event: {:?}, window: {:?}", event, window_id);
        match event {
            WindowEvent::CloseRequested => {
                log::info!("close requested, exiting...");
                event_loop.exit();
            }

            WindowEvent::RedrawRequested => {
                if let Some(output_iden) = self.engine.output_id_for_window(&window_id).cloned() {
                    if let Err(err) = self.engine.render_output(&output_iden, Instant::now()) {
                        log::error!("failed to render {output_iden}: {err}");
                        event_loop.exit();
                    }
                }
            }

            WindowEvent::Resized(size) => {
                if let Some(output_iden) = self.engine.output_id_for_window(&window_id).cloned() {
                    if let Err(err) = self.engine.resize_output(&output_iden, size) {
                        log::error!("failed to resize {output_iden}: {err}");
                        event_loop.exit();
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let plan = self.engine.schedule(Instant::now());
        for output_iden in plan.redraw_outputs {
            if let Some(window) = self.engine.get_window(&output_iden) {
                window.request_redraw();
            }
        }

        match plan.next_wakeup {
            Some(next) => event_loop.set_control_flow(ControlFlow::WaitUntil(next)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn user_event(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::CtrlC => {
                log::info!("received ctrlc from the terminal, exiting....");
                event_loop.exit();
            }
        }
    }
}

fn hwnd_from_window(window: &Arc<Window>) -> color_eyre::Result<windows::Win32::Foundation::HWND> {
    let hwnd = if let RawWindowHandle::Win32(h) = window.window_handle()?.window_handle()?.as_raw()
    {
        windows::Win32::Foundation::HWND(h.hwnd.get() as _)
    } else {
        color_eyre::eyre::bail!("No HWND available")
    };

    Ok(hwnd)
}

fn log_window_state(label: &str, hwnd: windows::Win32::Foundation::HWND) {
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let parent = GetParent(hwnd);

        let mut class_buf = vec![0u16; 256];
        let class_len = GetClassNameW(hwnd, &mut class_buf) as usize;
        let class_name = String::from_utf16_lossy(&class_buf[..class_len]);

        eprintln!(
            "{label}: hwnd={hwnd:?} parent={parent:?} class={class_name} style=0x{style:016X} exstyle=0x{exstyle:016X}"
        );
    }
}

fn install_ctrl_c_handler(proxy: EventLoopProxy<UserEvent>) {
    ctrlc::set_handler(move || {
        let _ = proxy.send_event(UserEvent::CtrlC);
    })
    .expect("failed to handle stop handler");
}

#[derive(Debug, Clone)]
pub enum UserEvent {
    CtrlC,
}
