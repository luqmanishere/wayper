//! Code handling WGPU stuff
//!

use std::sync::{
    Arc,
    mpsc::{Receiver, Sender},
};
use std::time::Instant;

use wgpu::naga::FastHashMap;
use winit::window::Window;

use crate::renderer::output::Output;

mod output;

pub struct Renderer {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    animation_start: Instant,
    last_render_time: Instant,

    output_map: FastHashMap<String, Output>,
}

impl Renderer {
    /// Create a new instance of the renderer
    pub async fn new() -> (wgpu::Instance, Sender<RendererAction>) {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .unwrap();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .unwrap();

        let (main_tx, main_rx) = std::sync::mpsc::channel();

        let renderer = Self {
            instance: instance.clone(),
            adapter,
            device,
            queue,
            animation_start: Instant::now(),
            last_render_time: Instant::now(),
            output_map: Default::default(),
        };

        // TODO: spawn thread
        log::info!("Starting rendering thread...");
        std::thread::spawn(move || {
            Self::worker(renderer, main_rx);
        });

        (instance, main_tx)
    }

    fn worker(mut renderer: Self, main_rx: Receiver<RendererAction>) {
        while let Ok(thing) = main_rx.recv() {
            let name = thing.to_string();
            let res = match thing {
                RendererAction::NewSurface {
                    output_iden,
                    size,
                    window_arc,
                } => renderer.new_surface(output_iden, size, window_arc),
                RendererAction::ResizeSurface {
                    output_iden,
                    new_size,
                } => {
                    renderer.resize_surface(output_iden, new_size);
                    Ok(())
                }
                RendererAction::RenderFrame { output_iden } => renderer.render_frame(output_iden),
            };

            if let Err(e) = res {
                log::error!("Error in rendering thread for request {name}: {e}");
            }
        }
    }
}

impl Renderer {
    pub fn new_surface(
        &mut self,
        output_iden: String,
        size: (u32, u32),
        window_arc: Arc<Window>,
    ) -> color_eyre::Result<()> {
        let surface = create_new_surface(self.instance.clone(), window_arc.clone())?;
        self.configure_surface(&surface, size);

        let cap = surface.get_capabilities(&self.adapter);
        let surface_format = cap.formats[0];
        let window_id = window_arc.id();

        self.output_map.insert(
            output_iden.clone(),
            Output {
                name: output_iden.clone(),
                size,
                surface,
                surface_format,
                window_arc,
            },
        );
        log::info!("surface created for output {output_iden}, window id {window_id:?}");

        Ok(())
    }

    pub fn render_frame(&mut self, output_iden: String) -> color_eyre::Result<()> {
        let output = self
            .output_map
            .get(&output_iden)
            .expect("surface initialized");
        let elapsed = self.animation_start.elapsed().as_secs_f64();
        let animated_color = wgpu::Color {
            r: ((elapsed * 0.7).sin() + 1.0) * 0.5,
            g: ((elapsed * 1.1 + 2.0).sin() + 1.0) * 0.5,
            b: ((elapsed * 1.6 + 4.0).sin() + 1.0) * 0.5,
            a: 1.0,
        };
        let surface = &output.surface;
        let surface_texture = surface
            .get_current_texture()
            .expect("failed to acquire next swapchain texture");
        let texture_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor {
                format: Some(output.surface_format.add_srgb_suffix()),
                ..Default::default()
            });

        let mut encoder = self.device.create_command_encoder(&Default::default());
        // Create the renderpass which will clear the screen.
        let renderpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &texture_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(animated_color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        drop(renderpass);

        // Submit the command in the queue to execute
        self.queue.submit([encoder.finish()]);
        output.window_arc.pre_present_notify();
        surface_texture.present();
        output.window_arc.request_redraw();
        let now = Instant::now();
        self.last_render_time = now;
        Ok(())
    }

    /// Resize a surface to the new output size
    pub fn resize_surface(&mut self, output_iden: String, new_size: (u32, u32)) {
        // take that, borrow checker!
        {
            if let Some(output) = self.output_map.get_mut(&output_iden) {
                output.size = new_size;
            }
        }
        {
            if let Some(output) = self.output_map.get(&output_iden) {
                let surface = &output.surface;
                self.configure_surface(surface, new_size);
            }
        }
    }

    pub fn configure_surface(&self, surface: &wgpu::Surface, size: (u32, u32)) {
        let cap = surface.get_capabilities(&self.adapter);
        let surface_format = cap.formats[0];

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            // Request compatibility with the sRGB-format texture view we‘re going to create later.
            view_formats: vec![surface_format.add_srgb_suffix()],
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            width: size.0,
            height: size.1,
            desired_maximum_frame_latency: 2,
            present_mode: wgpu::PresentMode::AutoVsync,
        };
        surface.configure(&self.device, &surface_config);
    }
}

#[derive(Debug)]
pub enum RendererAction {
    NewSurface {
        output_iden: String,
        size: (u32, u32),
        window_arc: Arc<Window>,
    },
    ResizeSurface {
        output_iden: String,
        new_size: (u32, u32),
    },
    RenderFrame {
        output_iden: String,
    },
}

impl std::fmt::Display for RendererAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            RendererAction::NewSurface { .. } => "NewSurface",
            RendererAction::ResizeSurface { .. } => "ResizeSurface",
            RendererAction::RenderFrame { .. } => "RenderFrame",
        };
        f.write_str(s)
    }
}

pub fn create_new_surface(
    instance: wgpu::Instance,
    window: Arc<winit::window::Window>,
) -> color_eyre::Result<wgpu::Surface<'static>> {
    #[cfg(not(target_os = "windows"))]
    let surface = instance.create_surface(window)?;

    #[cfg(target_os = "windows")]
    let surface = unsafe {
        use wgpu::rwh::HasDisplayHandle;
        use winit::platform::windows::WindowExtWindows;
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: window.display_handle()?.as_raw(),
            raw_window_handle: window.window_handle_any_thread()?.as_raw(),
        })
    }?;
    Ok(surface)
}
