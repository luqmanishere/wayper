use std::sync::Arc;

use color_eyre::{Result, eyre::OptionExt};
use image::RgbaImage;
use wgpu::naga::FastHashMap;
use winit::window::Window;

use crate::renderer::output::Output;

mod output;
const TEXTURED_FULLSCREEN_SHADER: &str = include_str!("image.wgsl");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureHandle(u64);

#[derive(Debug, Clone, Copy)]
pub enum FitMode {
    Contain,
    Cover,
    Stretch,
}

#[derive(Debug, Clone, Copy)]
pub struct FitAlignment {
    pub x: f32,
    pub y: f32,
}

pub enum RenderScene {
    Image {
        texture: TextureHandle,
        image_size: (u32, u32),
        fit: FitMode,
        alignment: FitAlignment,
    },
}

struct RendererTexture {
    #[expect(unused)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

pub struct Renderer {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    output_map: FastHashMap<String, Output>,
    textures: FastHashMap<TextureHandle, RendererTexture>,
    next_texture_id: u64,
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

impl Renderer {
    pub async fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await?;

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("wallpaper_image_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("wallpaper_texture_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wallpaper_image_shader"),
            source: wgpu::ShaderSource::Wgsl(TEXTURED_FULLSCREEN_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("wallpaper_render_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wallpaper_render_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Bgra8UnormSrgb,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            output_map: Default::default(),
            textures: Default::default(),
            next_texture_id: 1,
            bind_group_layout,
            pipeline,
            sampler,
        })
    }

    /// Create a new surface for a window
    pub fn create_surface(
        &mut self,
        output_iden: String,
        size: (u32, u32),
        window_arc: Arc<Window>,
    ) -> Result<()> {
        let surface = create_new_surface(self.instance.clone(), window_arc.clone())?;
        let surface_config = self.surface_config_for(&surface, size)?;
        let surface_format = surface_config.format;
        surface.configure(&self.device, &surface_config);

        self.output_map.insert(
            output_iden,
            Output {
                surface,
                config: surface_config,
                surface_format,
                window_arc,
            },
        );
        Ok(())
    }

    /// Resize a surface to the provided size. Error if the surface does not exist.
    pub fn resize_surface(&mut self, output_iden: &str, new_size: (u32, u32)) -> Result<()> {
        let output = self
            .output_map
            .get_mut(output_iden)
            .ok_or_eyre("surface must exist before resizing")?;

        if new_size.0 == 0 || new_size.1 == 0 {
            return Ok(());
        }

        output.config.width = new_size.0;
        output.config.height = new_size.1;
        output.surface.configure(&self.device, &output.config);
        Ok(())
    }

    pub fn upload_rgba_image(
        &mut self,
        rgba: &RgbaImage,
        dimensions: (u32, u32),
    ) -> Result<TextureHandle> {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("wallpaper_image_texture"),
            size: wgpu::Extent3d {
                width: dimensions.0,
                height: dimensions.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba.as_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(dimensions.0 * 4),
                rows_per_image: Some(dimensions.1),
            },
            wgpu::Extent3d {
                width: dimensions.0,
                height: dimensions.1,
                depth_or_array_layers: 1,
            },
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let handle = TextureHandle(self.next_texture_id);
        self.next_texture_id += 1;
        self.textures
            .insert(handle, RendererTexture { texture, view });

        Ok(handle)
    }

    pub fn render(&mut self, output_iden: &str, scene: &RenderScene) -> Result<()> {
        let output = self
            .output_map
            .get(output_iden)
            .ok_or_eyre("surface must exist before rendering")?;
        let surface_texture = output.surface.get_current_texture()?;
        let texture_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor {
                format: Some(output.surface_format.add_srgb_suffix()),
                ..Default::default()
            });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("wallpaper_render_encoder"),
            });

        let (texture, image_size, fit, alignment) = match scene {
            RenderScene::Image {
                texture,
                image_size,
                fit,
                alignment,
            } => (
                self.textures
                    .get(texture)
                    .ok_or_eyre("scene texture must be uploaded before rendering")?,
                *image_size,
                *fit,
                *alignment,
            ),
        };
        let image_params = self.image_params(
            (output.config.width, output.config.height),
            image_size,
            fit,
            alignment,
        );
        let image_params_bytes = unsafe {
            std::slice::from_raw_parts(
                image_params.as_ptr().cast::<u8>(),
                std::mem::size_of_val(&image_params),
            )
        };
        let image_params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wallpaper_image_params_buffer"),
            size: image_params_bytes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&image_params_buffer, 0, image_params_bytes);
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("wallpaper_image_bind_group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: image_params_buffer.as_entire_binding(),
                },
            ],
        });

        {
            let mut renderpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("wallpaper_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &texture_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            renderpass.set_pipeline(&self.pipeline);
            renderpass.set_bind_group(0, &bind_group, &[]);
            renderpass.draw(0..3, 0..1);
        }

        self.queue.submit([encoder.finish()]);
        output.window_arc.pre_present_notify();
        surface_texture.present();
        Ok(())
    }

    /// Produce a configuraton for the surface, given the size
    fn surface_config_for(
        &self,
        surface: &wgpu::Surface,
        size: (u32, u32),
    ) -> Result<wgpu::SurfaceConfiguration> {
        let cap = surface.get_capabilities(&self.adapter);
        let surface_format = cap
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .or_else(|| cap.formats.first().copied())
            .ok_or_eyre("surface has no supported formats")?;

        Ok(wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            view_formats: vec![surface_format.add_srgb_suffix()],
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            width: size.0.max(1),
            height: size.1.max(1),
            desired_maximum_frame_latency: 2,
            present_mode: wgpu::PresentMode::AutoVsync,
        })
    }

    fn image_params(
        &self,
        output_size: (u32, u32),
        image_size: (u32, u32),
        fit: FitMode,
        alignment: FitAlignment,
    ) -> [f32; 4] {
        let output_aspect = output_size.0 as f32 / output_size.1.max(1) as f32;
        let image_aspect = image_size.0 as f32 / image_size.1.max(1) as f32;

        match fit {
            FitMode::Stretch => [1.0, 1.0, 0.0, 0.0],
            FitMode::Cover => {
                if output_aspect > image_aspect {
                    let scale_y = image_aspect / output_aspect;
                    [1.0, scale_y, 0.0, (1.0 - scale_y) * alignment.y]
                } else {
                    let scale_x = output_aspect / image_aspect;
                    [scale_x, 1.0, (1.0 - scale_x) * alignment.x, 0.0]
                }
            }
            FitMode::Contain => {
                if output_aspect > image_aspect {
                    let content_scale_x = image_aspect / output_aspect;
                    [
                        1.0 / content_scale_x,
                        1.0,
                        -((1.0 / content_scale_x) - 1.0) * alignment.x,
                        0.0,
                    ]
                } else {
                    let content_scale_y = output_aspect / image_aspect;
                    [
                        1.0,
                        1.0 / content_scale_y,
                        0.0,
                        -((1.0 / content_scale_y) - 1.0) * alignment.y,
                    ]
                }
            }
        }
    }
}

pub fn create_new_surface(
    instance: wgpu::Instance,
    window: Arc<winit::window::Window>,
) -> Result<wgpu::Surface<'static>> {
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
