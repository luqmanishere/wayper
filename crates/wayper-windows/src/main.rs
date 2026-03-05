use std::sync::{Arc, mpsc::Sender};

use env_logger::Env;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use wgpu::naga::FastHashMap;
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetParent, GetWindowLongPtrW,
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ControlFlow, EventLoop, EventLoopProxy},
    window::{Window, WindowId},
};
use wayper_windows::windows_host::{
    find_shelldll_defview, get_progman, reparent_window, set_z_pos, spawn_workerw,
};

use crate::renderer::{Renderer, RendererAction};

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

    let mut app = App::new();
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    install_ctrl_c_handler(proxy);
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct App {
    #[expect(unused)]
    wgpu_instance: wgpu::Instance,
    renderer_tx: Sender<RendererAction>,
    windows: FastHashMap<String, Arc<Window>>,
    window_id_iden_map: FastHashMap<WindowId, String>,
}

impl App {
    pub fn new() -> Self {
        let (wgpu_instance, renderer_tx) = pollster::block_on(Renderer::new());
        Self {
            wgpu_instance,
            renderer_tx,
            windows: Default::default(),
            window_id_iden_map: Default::default(),
        }
    }

    #[expect(unused)]
    fn get_window_from_id(&self, window_id: &WindowId) -> Option<&Arc<Window>> {
        if let Some(output_iden) = self.window_id_iden_map.get(window_id) {
            self.windows.get(output_iden)
        } else {
            None
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        log::debug!("resumed called, checking for new windows to spawn");

        for mon in event_loop.available_monitors() {
            let output_iden = mon.name().unwrap_or("Unknown".to_string());
            if self.windows.get(&output_iden).is_none() {
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
                let window_id = window.id();
                log::info!("window {:?} created", window_id);

                self.windows.insert(output_iden.clone(), window.clone());
                self.window_id_iden_map
                    .insert(window_id, output_iden.clone());

                // windows stuff
                let progman = get_progman().unwrap();
                let workerw = spawn_workerw(progman).unwrap();
                let shelldll = find_shelldll_defview(progman).unwrap();
                log_window_state("main pre-attach", hwnd_from_window(&window).unwrap());
                reparent_window(progman,  window.clone()).unwrap();
                set_z_pos(shelldll, workerw, window.clone()).unwrap();
                log_window_state("main post-attach", hwnd_from_window(&window).unwrap());

                self.renderer_tx
                    .send(RendererAction::NewSurface {
                        output_iden,
                        size: (size.width, size.height),
                        window_arc: window.clone(),
                    })
                    .expect("able to send");

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
                // if let Some(window) = self.get_window_from_id(&window_id) {
                //     let hwnd = hwnd_from_window(window).unwrap();
                //     log_window_state("main redraw", hwnd);
                // }

                if let Some(output_iden) = self.window_id_iden_map.get(&window_id) {
                    self.renderer_tx
                        .send(RendererAction::RenderFrame {
                            output_iden: output_iden.to_string(),
                        })
                        .expect("channel alive");
                }

                // self.get_window_from_id(&window_id)
                //     .expect("initialized")
                //     .request_redraw();
            }

            WindowEvent::Resized(size) => {
                if let Some(output_iden) = self.window_id_iden_map.get(&window_id) {
                    self.renderer_tx
                        .send(RendererAction::ResizeSurface {
                            output_iden: output_iden.to_string(),
                            new_size: (size.width, size.height),
                        })
                        .expect("able to send");
                }
            }
            _ => {}
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

//     fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
//         self.size = new_size;

//         // reconfigure the surface
//         self.configure_surface();
//     }
