//! The wgpu renderer
//! TODO: heavily comment this arcane piece of shit
use std::{path::Path, ptr::NonNull};

use image::GenericImageView;
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use wgpu::{naga::FastHashMap, util::DeviceExt};

pub struct WgpuRenderer {
    instance: wgpu::Instance,
    pub adapter: Option<wgpu::Adapter>,
    pub device: Option<wgpu::Device>,
    pub queue: Option<wgpu::Queue>,
    pub map: FastHashMap<String, wgpu::Surface<'static>>,
    pub image_pipeline: Option<wgpu::RenderPipeline>,
    pub bind_group_layout: Option<wgpu::BindGroupLayout>,
    pub vertex_buffer: Option<wgpu::Buffer>,
    pub index_buffer: Option<wgpu::Buffer>,
    pub sampler: Option<wgpu::Sampler>,
    pub transition_params_buf: Option<wgpu::Buffer>,
    /// Texture management - cache textures by image path + size
    pub texture_cache: FastHashMap<String, wgpu::Texture>,
    pub bind_group_cache: FastHashMap<String, wgpu::BindGroup>,
    pub surface_configs: FastHashMap<String, wgpu::SurfaceConfiguration>,
    /// Track current image cache key per output for transitions (output_name -> cache_key)
    pub current_image_keys: FastHashMap<String, String>,
    // TODO: proper keys
}

impl WgpuRenderer {
    pub fn new() -> Self {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            // TODO: support for other platforms via winit for debugging
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        Self {
            instance,
            adapter: None,
            device: None,
            queue: None,
            map: Default::default(),
            image_pipeline: None,
            bind_group_layout: None,
            vertex_buffer: None,
            index_buffer: None,
            sampler: None,
            transition_params_buf: None,
            texture_cache: Default::default(),
            bind_group_cache: Default::default(),
            surface_configs: Default::default(),
            current_image_keys: Default::default(),
        }
    }

    pub fn new_surface(
        &mut self,
        output_name: String,
        display: *mut wayland_sys::client::wl_display,
        surfac: *mut wayland_sys::client::wl_proxy,
    ) -> color_eyre::Result<()> {
        let raw_display_handle = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            NonNull::new(display as *mut _).unwrap(),
        ));
        let raw_window_handle = RawWindowHandle::Wayland(WaylandWindowHandle::new(
            NonNull::new(surfac as *mut _).unwrap(),
        ));

        let surface = unsafe {
            self.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle,
                    raw_window_handle,
                })
        }?;
        if self.adapter.is_none() {
            let adapter = pollster::block_on(self.instance.request_adapter(
                &wgpu::RequestAdapterOptionsBase {
                    compatible_surface: Some(&surface),
                    ..Default::default()
                },
            ))?;
            let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))?;
            self.adapter = Some(adapter);
            self.device = Some(device);
            self.queue = Some(queue);
        }

        self.map.insert(output_name, surface);
        Ok(())
    }

    pub fn init_image_pipeline(
        &mut self,
        surface_format: wgpu::TextureFormat,
    ) -> color_eyre::Result<()> {
        let device = self
            .device
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Device not initialized"))?;

        if self.image_pipeline.is_some() {
            return Ok(());
        }

        // Load shader
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Image Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/shader.wgsl").into()),
        });

        // Create bind group layout for texture and sampler
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Image Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
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

        // Create pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Image Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create render pipeline
        let image_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Image Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<[f32; 4]>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        });

        // Create quad vertices as flat array (position and UV coordinates)
        let vertices: &[f32] = &[
            // bottom-left: pos(-1, -1), uv(0, 1)
            -1.0, -1.0, 0.0, 1.0, // bottom-right: pos(1, -1), uv(1, 1)
            1.0, -1.0, 1.0, 1.0, // top-right: pos(1, 1), uv(1, 0)
            1.0, 1.0, 1.0, 0.0, // top-left: pos(-1, 1), uv(0, 0)
            -1.0, 1.0, 0.0, 0.0,
        ];

        let indices = [0u16, 1, 2, 0, 2, 3];

        let transition_params = TransitionParams {
            progress: 0.0,
            anim_type: 0,
            direction: [0.0, 0.0],
        };

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Vertex Buffer"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Index Buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let transition_params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("transition params buf"),
            contents: bytemuck::bytes_of(&transition_params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        self.image_pipeline = Some(image_pipeline);
        self.bind_group_layout = Some(bind_group_layout);
        self.vertex_buffer = Some(vertex_buffer);
        self.index_buffer = Some(index_buffer);
        self.transition_params_buf = Some(transition_params_buf);
        self.sampler = Some(sampler);

        println!("Image pipeline initialized successfully");
        Ok(())
    }

    /// Generate cache key from image path and target size
    fn cache_key(image_path: &Path, target_size: (u32, u32)) -> String {
        format!(
            "{}@{}x{}",
            image_path.display(),
            target_size.0,
            target_size.1
        )
    }

    /// Generate cache key for dummy black texture
    fn dummy_texture_key(target_size: (u32, u32)) -> String {
        format!("__dummy_black__@{}x{}", target_size.0, target_size.1)
    }

    /// Get or create a black dummy texture of the specified size
    pub fn get_or_create_dummy_texture(
        &mut self,
        target_size: (u32, u32),
    ) -> color_eyre::Result<String> {
        let cache_key = Self::dummy_texture_key(target_size);

        // Check if dummy texture is already cached
        if self.texture_cache.contains_key(&cache_key) {
            return Ok(cache_key);
        }

        let device = self
            .device
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Device not initialized"))?;
        let queue = self
            .queue
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Queue not initialized"))?;

        // Create black image data
        let black_data = vec![0u8; (target_size.0 * target_size.1 * 4) as usize];

        let texture_size = wgpu::Extent3d {
            width: target_size.0,
            height: target_size.1,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Dummy Black Texture"),
            size: texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Write black data to the texture
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &black_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * target_size.0),
                rows_per_image: Some(target_size.1),
            },
            texture_size,
        );

        // Cache the texture
        self.texture_cache.insert(cache_key.clone(), texture);
        Ok(cache_key)
    }

    /// Load an image and create a wgpu texture from it
    pub fn load_image_texture(
        &mut self,
        image_path: &Path,
        target_size: (u32, u32),
    ) -> color_eyre::Result<&wgpu::Texture> {
        let device = self
            .device
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Device not initialized"))?;
        let queue = self
            .queue
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Queue not initialized"))?;

        let cache_key = Self::cache_key(image_path, target_size);

        // Check if texture is already cached
        if self.texture_cache.contains_key(&cache_key) {
            return Ok(self.texture_cache.get(&cache_key).unwrap());
        }

        // Load and resize the image
        let img = image::open(image_path)?.resize_to_fill(
            target_size.0,
            target_size.1,
            image::imageops::FilterType::Lanczos3,
        );
        let rgba = img.to_rgba8();
        let (img_width, img_height) = img.dimensions();

        let texture_size = wgpu::Extent3d {
            width: img_width,
            height: img_height,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Image Texture"),
            size: texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Write the image data to the texture
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * img_width),
                rows_per_image: Some(img_height),
            },
            texture_size,
        );

        // Cache the texture
        self.texture_cache.insert(cache_key.clone(), texture);
        Ok(self.texture_cache.get(&cache_key).unwrap())
    }

    /// Get or create a bind group for two textures
    pub fn get_or_create_bind_group(
        &mut self,
        cache_key1: &str,
        cache_key2: &str,
    ) -> color_eyre::Result<String> {
        // Generate bind group cache key from both texture keys
        let bind_group_key = format!("{}+{}", cache_key1, cache_key2);

        // Check if bind group is already cached
        if self.bind_group_cache.contains_key(&bind_group_key) {
            return Ok(bind_group_key);
        }

        let device = self
            .device
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Device not initialized"))?;
        let bind_group_layout = self
            .bind_group_layout
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Bind group layout not initialized"))?;
        let sampler = self
            .sampler
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Sampler not initialized"))?;
        let texture1 = self.texture_cache.get(cache_key1).ok_or_else(|| {
            color_eyre::eyre::eyre!("Texture 1 not found in cache: {}", cache_key1)
        })?;
        let texture2 = self.texture_cache.get(cache_key2).ok_or_else(|| {
            color_eyre::eyre::eyre!("Texture 2 not found in cache: {}", cache_key2)
        })?;
        let transition_params = self
            .transition_params_buf
            .as_ref()
            .expect("buffer initialized")
            .as_entire_buffer_binding();

        let texture_view1 = texture1.create_view(&wgpu::TextureViewDescriptor::default());
        let texture_view2 = texture2.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                // tex1 - previous texture
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&texture_view1),
                },
                // tex2 - current texture
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&texture_view2),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Buffer(transition_params),
                },
            ],
            label: Some("Image Bind Group"),
        });

        // Cache the bind group
        self.bind_group_cache
            .insert(bind_group_key.clone(), bind_group);
        Ok(bind_group_key)
    }

    /// Update transition parameters for animations
    pub fn update_transition_params(
        &mut self,
        progress: f32,
        anim_type: u32,
        direction: [f32; 2],
    ) -> color_eyre::Result<()> {
        let queue = self
            .queue
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Queue not initialized"))?;
        let transition_params_buf = self
            .transition_params_buf
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Transition params buffer not initialized"))?;

        let transition_params = TransitionParams {
            progress,
            anim_type,
            direction,
        };

        queue.write_buffer(
            transition_params_buf,
            0,
            bytemuck::bytes_of(&transition_params),
        );
        Ok(())
    }

    /// Configure surface for a specific output
    pub fn configure_surface(
        &mut self,
        output_name: &str,
        size: (u32, u32),
    ) -> color_eyre::Result<wgpu::TextureFormat> {
        let adapter = self
            .adapter
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Adapter not initialized"))?;
        let device = self
            .device
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Device not initialized"))?;
        let surface = self.map.get(output_name).ok_or_else(|| {
            color_eyre::eyre::eyre!("Surface not found for output: {}", output_name)
        })?;

        let caps = surface.get_capabilities(adapter);
        let surface_format = caps.formats[0];

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            view_formats: vec![surface_format],
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            width: size.0,
            height: size.1,
            desired_maximum_frame_latency: 2,
            present_mode: wgpu::PresentMode::Mailbox,
        };

        surface.configure(device, &config);
        self.surface_configs.insert(output_name.to_string(), config);

        Ok(surface_format)
    }

    /// Render an image to a specific output with transition support
    pub fn render_to_output(
        &mut self,
        output_name: &str,
        image_path: &Path,
    ) -> color_eyre::Result<()> {
        // Get target size first
        let target_size = {
            let surface_config = self.surface_configs.get(output_name).ok_or_else(|| {
                color_eyre::eyre::eyre!("Surface config not found for output: {}", output_name)
            })?;
            (surface_config.width, surface_config.height)
        };

        // Load the new image texture
        let current_cache_key = Self::cache_key(image_path, target_size);
        self.load_image_texture(image_path, target_size)?;

        // Get or create previous texture (either actual previous or dummy black)
        let previous_cache_key = self
            .current_image_keys
            .get(output_name)
            .cloned()
            .unwrap_or_else(|| {
                // First image on this output - create dummy texture
                self.get_or_create_dummy_texture(target_size)
                    .expect("Failed to create dummy texture")
            });

        // Create bind group with (previous, current) textures
        let bind_group_key =
            self.get_or_create_bind_group(&previous_cache_key, &current_cache_key)?;

        // Reset progress to 1.0 to show the current image (tex2) fully
        self.update_transition_params(1.0, 0, [0.0, 0.0])?;

        // Update tracking for next transition
        self.current_image_keys
            .insert(output_name.to_string(), current_cache_key);

        // Get references for rendering
        let device = self
            .device
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Device not initialized"))?;
        let queue = self
            .queue
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Queue not initialized"))?;
        let render_pipeline = self
            .image_pipeline
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Image pipeline not initialized"))?;
        let vertex_buffer = self
            .vertex_buffer
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Vertex buffer not initialized"))?;
        let index_buffer = self
            .index_buffer
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Index buffer not initialized"))?;
        let surface = self.map.get(output_name).ok_or_else(|| {
            color_eyre::eyre::eyre!("Surface not found for output: {}", output_name)
        })?;
        let bind_group = self
            .bind_group_cache
            .get(&bind_group_key)
            .ok_or_else(|| color_eyre::eyre::eyre!("Bind group not found in cache"))?;

        // Get surface texture and render
        let surface_texture = surface.get_current_texture()?;
        let surface_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Image Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &surface_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            render_pass.set_pipeline(render_pipeline);
            render_pass.set_bind_group(0, bind_group, &[]);
            render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint16);
            render_pass.draw_indexed(0..6, 0, 0..1);
        }

        queue.submit(Some(encoder.finish()));
        surface_texture.present();

        Ok(())
    }

    /// Handle frame rendering for a specific output - called from frame callback
    pub fn handle_frame(&mut self, output_name: &str, image_path: &Path) -> color_eyre::Result<()> {
        self.render_to_output(output_name, image_path)
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TransitionParams {
    progress: f32,
    anim_type: u32,
    direction: [f32; 2],
}
