//! The wgpu renderer
//! TODO: heavily comment this arcane piece of shit
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    ptr::NonNull,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, Sender},
    },
    time::Instant,
};

use color_eyre::eyre::eyre;
use image::{GenericImageView, imageops::FilterType::Lanczos3};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use wayper_lib::socket::GpuMetricsData;
use wgpu::{naga::FastHashMap, util::DeviceExt};

const FIT_MODE_COVER: u32 = 2;
const DEFAULT_BACKGROUND: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// RAII timer for GPU operations - logs elapsed time on drop
struct GpuOperationTimer {
    start: Instant,
    operation: &'static str,
}

impl GpuOperationTimer {
    fn new(operation: &'static str) -> Self {
        Self {
            start: Instant::now(),
            operation,
        }
    }
}

impl Drop for GpuOperationTimer {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        tracing::trace!(
            operation = self.operation,
            time_us = elapsed.as_micros(),
            "GPU operation completed"
        );
    }
}

/// A cached texture, with its height and width encoded.
pub struct CachedTexture {
    texture: wgpu::Texture,
    width: u32,
    height: u32,
}

/// Main Renderer struct
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
    pub render_params_buf: Option<wgpu::Buffer>,
    /// Texture management - cache textures by image path + size
    pub texture_cache: crate::metered_cache::MeteredCache<String, CachedTexture>,
    pub bind_group_cache: crate::metered_cache::MeteredCache<String, wgpu::BindGroup>,
    pub surface_configs: FastHashMap<String, wgpu::SurfaceConfiguration>,

    texture_loader_tx: Sender<TextureLoadRequest>,
    texture_loader_rx: Receiver<TextureLoadResult>,
    in_flight_texture_loads: HashSet<String>,

    /// Total frames rendered for metrics
    pub total_frames_rendered: AtomicU64,
    // TODO: proper keys
}

impl WgpuRenderer {
    /// Create a new instance of the renderer
    pub fn new() -> (Sender<RenderCommand>, wgpu::Instance) {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            // TODO: support for other platforms via winit for debugging
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let (load_tx, load_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();

        let (command_tx, command_rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            texture_loader_worker(load_rx, result_tx);
        });

        // send the instance into a seperate thread
        let instance1 = instance.clone();
        std::thread::spawn(move || {
            let mut renderer = Self {
                instance: instance1,
                adapter: None,
                device: None,
                queue: None,
                map: Default::default(),
                image_pipeline: None,
                bind_group_layout: None,
                vertex_buffer: None,
                index_buffer: None,
                sampler: None,
                render_params_buf: None,
                texture_cache: crate::metered_cache::MeteredCache::new(
                    std::num::NonZeroUsize::new(10).expect("non-zero"),
                ),
                bind_group_cache: crate::metered_cache::MeteredCache::new(
                    std::num::NonZeroUsize::new(20).expect("non-zero"),
                ),
                surface_configs: Default::default(),
                texture_loader_tx: load_tx,
                texture_loader_rx: result_rx,
                in_flight_texture_loads: Default::default(),
                total_frames_rendered: AtomicU64::new(0),
            };

            renderer.worker(command_rx);
        });

        (command_tx, instance)
    }

    /// worker thread: processes render commands
    fn worker(&mut self, command_rx: Receiver<RenderCommand>) {
        while let Ok(command) = command_rx.recv() {
            let command_name = command.to_string();
            let process_start = Instant::now();
            if let Ok(count) = self.process_loaded_textures()
                && count > 0
            {
                let process_time = process_start.elapsed();
                tracing::debug!(
                    count = count,
                    time_us = process_time.as_micros(),
                    "Processed pre-loaded textures"
                );
            }

            // TODO: error handling
            match self.handle_command(command) {
                Ok(_) => {
                    // TODO: logging
                }
                Err(e) => {
                    tracing::error!(
                        "Error encountered while processing command {command_name}: {e}",
                    )
                }
            };
        }
    }

    /// parse and handle commands
    fn handle_command(&mut self, command: RenderCommand) -> color_eyre::Result<()> {
        match command {
            RenderCommand::NewSurface {
                output_name,
                surface,
            } => {
                self.new_surface(output_name, surface)?;
            }
            RenderCommand::ConfigureSurface { output_name, size } => {
                self.configure_surface(&output_name, size)?;
            }
            RenderCommand::RequestTextureLoad {
                image_path,
                output_name,
            } => {
                self.request_texture_load(image_path.as_path(), output_name)?;
            }
            RenderCommand::RenderFrame {
                output_name,
                previous_image,
                current_image,
                progress,
                transition_type,
                direction,
                fit_mode,
            } => {
                self.render_frame(
                    &output_name,
                    previous_image.as_deref(),
                    current_image.as_path(),
                    progress,
                    transition_type,
                    direction,
                    fit_mode,
                )?;
            }
            RenderCommand::LogCacheMetrics => {
                self.log_cache_metrics();
            }
            RenderCommand::GetMetricsData { reply } => {
                reply.send(self.get_metrics_data())?;
            }
        }

        Ok(())
    }
}

impl WgpuRenderer {
    /// Load an image texture from a specified path for a a target size. NoOp if the texture is already cached,
    /// otherwise the image is loaded and resized to the target size.
    fn request_texture_load(
        &mut self,
        image_path: &Path,
        output_name: String,
    ) -> color_eyre::Result<()> {
        let cache_key = Self::cache_key(image_path);

        if self.texture_cache.contains(&cache_key)
            || self.in_flight_texture_loads.contains(&cache_key)
        {
            return Ok(());
        }

        let max_2d = if let Some(device) = &self.device {
            device.limits().max_texture_dimension_2d
        } else {
            // This is the max reported by my system
            8192
        };

        self.in_flight_texture_loads.insert(cache_key);

        if let Err(e) = self.texture_loader_tx.send(TextureLoadRequest {
            image_path: image_path.to_path_buf(),
            output_name,
            max_2d,
        }) {
            self.in_flight_texture_loads
                .remove(&Self::cache_key(image_path));
            return Err(e.into());
        }

        Ok(())
    }

    /// Process loaded textures returned by the texture loader
    #[tracing::instrument(skip(self))]
    fn process_loaded_textures(&mut self) -> color_eyre::Result<usize> {
        let device = self
            .device
            .as_ref()
            .ok_or_else(|| eyre!("Device not initialized"))?;
        let queue = self
            .queue
            .as_ref()
            .ok_or_else(|| eyre!("Queue not initialized"))?;

        let mut count = 0;

        while let Ok(result) = self.texture_loader_rx.try_recv() {
            let (cache_key, image_data, dimensions) = match result {
                TextureLoadResult::Loaded {
                    cache_key,
                    image_data,
                    dimensions,
                } => (cache_key, image_data, dimensions),
                TextureLoadResult::Failed { cache_key } => {
                    self.in_flight_texture_loads.remove(&cache_key);
                    continue;
                }
            };

            self.in_flight_texture_loads.remove(&cache_key);

            if self.texture_cache.contains(&cache_key) {
                continue;
            }

            let texture_size = wgpu::Extent3d {
                width: dimensions.0,
                height: dimensions.1,
                depth_or_array_layers: 1,
            };

            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("image texture"),
                size: texture_size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });

            write_texture_rgba8_padded(queue, &texture, dimensions.0, dimensions.1, &image_data);

            let size_bytes = Self::texture_size_bytes(dimensions.0, dimensions.1);
            self.texture_cache
                .get_or_insert(cache_key, size_bytes, || CachedTexture {
                    texture,
                    width: dimensions.0,
                    height: dimensions.1,
                });
            count += 1;
        }

        Ok(count)
    }
}

fn write_texture_rgba8_padded(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    rgba: &[u8],
) {
    let bytes_per_pixel = 4u32;
    let unpadded_bpr = width * bytes_per_pixel;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_bpr = ((unpadded_bpr + align - 1) / align) * align;

    if padded_bpr == unpadded_bpr {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(unpadded_bpr),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        return;
    }

    let row_bytes = unpadded_bpr as usize;
    let padded_row_bytes = padded_bpr as usize;
    let padded_size = padded_row_bytes.saturating_mul(height as usize);
    let mut padded = vec![0u8; padded_size];
    for y in 0..height as usize {
        let src = &rgba[y * row_bytes..(y + 1) * row_bytes];
        let dst = &mut padded[y * padded_row_bytes..y * padded_row_bytes + row_bytes];
        dst.copy_from_slice(src);
    }

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &padded,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(padded_bpr),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

/// Texture loader worker ran in a seperate thread. Results are pushed to the results channel
fn texture_loader_worker(
    load_rx: Receiver<TextureLoadRequest>,
    result_tx: Sender<TextureLoadResult>,
) {
    while let Ok(request) = load_rx.recv() {
        let span = tracing::span!(
            tracing::Level::DEBUG,
            "texture_load",
            output = %request.output_name,
            path = %request.image_path.display(),
            max_2d = %request.max_2d,
        );
        let _enter = span.enter();

        let load_start = Instant::now();
        match load_image_rgba(&request.image_path, request.max_2d) {
            Ok((image_data, dimensions)) => {
                let load_time = load_start.elapsed();
                tracing::debug!(
                    time_ms = load_time.as_millis(),
                    dimensions = format!("{}x{}", dimensions.0, dimensions.1),
                    "Image loaded"
                );
                let cache_key = WgpuRenderer::cache_key(&request.image_path);

                let _ = result_tx.send(TextureLoadResult::Loaded {
                    cache_key,
                    image_data,
                    dimensions,
                });
            }
            Err(e) => {
                tracing::error!("Failed to load image: {}", e);
                let _ = result_tx.send(TextureLoadResult::Failed {
                    cache_key: WgpuRenderer::cache_key(&request.image_path),
                });
            }
        }
    }
}

/// Load an image from a path as RGBA pixels at its original dimensions.
fn load_image_rgba(
    image_path: &Path,
    max_2d: u32,
) -> color_eyre::Result<(image::RgbaImage, (u32, u32))> {
    let img = {
        let mut img = image::open(image_path)?;
        let dim = img.dimensions();
        if dim.0 > max_2d || dim.1 > max_2d {
            img = img.resize(max_2d, max_2d, Lanczos3);
            tracing::debug!(
                "image {} is too big, resized to bounds {}",
                image_path.display(),
                max_2d
            );
        }
        img
    };
    let rgba = img.to_rgba8();
    let dimensions = rgba.dimensions();
    Ok((rgba, dimensions))
}

impl WgpuRenderer {
    /// Add a new surface to the renderer cache
    fn new_surface(
        &mut self,
        output_name: String,
        surface: wgpu::Surface<'static>,
    ) -> color_eyre::Result<()> {
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

    /// Build and initialize the image pipeline
    #[tracing::instrument(skip(self), fields(format = ?surface_format))]
    fn init_image_pipeline(
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

        let shader_str = format!(
            "{}\n{}",
            include_str!("../shaders/sizing.wgsl"),
            include_str!("../shaders/shader.wgsl")
        );

        // Load shader
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Image Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_str.into()),
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

        let transition_params = RenderParams {
            progress: 0.0,
            anim_type: 0,
            fit_mode: FIT_MODE_COVER,
            _pad0: 0,
            direction: [0.0, 0.0],
            output_size: [0.0, 0.0],
            background: DEFAULT_BACKGROUND,
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

        let render_params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("render params buf"),
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
        self.render_params_buf = Some(render_params_buf);
        self.sampler = Some(sampler);

        tracing::info!("Image pipeline initialized successfully");
        Ok(())
    }

    /// Generate cache key from image path and target size
    fn cache_key(image_path: &Path) -> String {
        format!("{}", image_path.display(),)
    }

    /// Generate cache key for dummy black texture
    fn dummy_texture_key(target_size: (u32, u32)) -> String {
        format!("__dummy_black__@{}x{}", target_size.0, target_size.1)
    }

    /// Calculate memory size of a texture in bytes (RGBA8 = 4 bytes per pixel)
    fn texture_size_bytes(width: u32, height: u32) -> u64 {
        (width as u64) * (height as u64) * 4
    }

    /// Get or create a black dummy texture of the specified size
    fn get_or_create_dummy_texture(
        &mut self,
        target_size: (u32, u32),
    ) -> color_eyre::Result<String> {
        let cache_key = Self::dummy_texture_key(target_size);

        // Check if dummy texture is already cached
        if self.texture_cache.contains(&cache_key) {
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

        // Cache the texture with size tracking
        let size_bytes = Self::texture_size_bytes(target_size.0, target_size.1);
        self.texture_cache
            .get_or_insert(cache_key.clone(), size_bytes, || CachedTexture {
                texture,
                width: target_size.0,
                height: target_size.1,
            });
        Ok(cache_key)
    }

    /// Load an image and create a wgpu texture from it
    #[tracing::instrument(skip(self),
        fields(path = %image_path.display(), size = format!("{}x{}", target_size.0, target_size.1)))]
    fn load_image_texture(
        &mut self,
        image_path: &Path,
        target_size: (u32, u32),
    ) -> color_eyre::Result<&CachedTexture> {
        let device = self
            .device
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Device not initialized"))?;
        let queue = self
            .queue
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Queue not initialized"))?;

        let cache_key = Self::cache_key(image_path);

        // Check if texture is already cached
        if self.texture_cache.contains(&cache_key) {
            tracing::trace!("Texture cache hit");
            // return early if already cached
            return Ok(self.texture_cache.get(&cache_key).unwrap());
        }

        tracing::trace!("Texture cache miss - loading from disk");

        let max_2d = device.limits().max_texture_dimension_2d;
        let (rgba, (img_width, img_height)) = load_image_rgba(image_path, max_2d)?;

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

        // Cache the texture with size tracking
        let size_bytes = Self::texture_size_bytes(img_width, img_height);
        Ok(self
            .texture_cache
            .get_or_insert(cache_key, size_bytes, || CachedTexture {
                texture,
                width: img_width,
                height: img_height,
            }))
    }

    /// Get or create a bind group for two textures
    fn get_or_create_bind_group(
        &mut self,
        cache_key1: &str,
        cache_key2: &str,
    ) -> color_eyre::Result<String> {
        // Generate bind group cache key from both texture keys
        let bind_group_key = format!("{}+{}", cache_key1, cache_key2);

        // Check if bind group is already cached
        if self.bind_group_cache.contains(&bind_group_key) {
            tracing::trace!("Bind group cache hit");
            return Ok(bind_group_key);
        }

        tracing::trace!("Bind group cache miss - creating new");

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
        // Use peek() to avoid mutable borrow issues when accessing multiple cache entries
        let texture1 = &self
            .texture_cache
            .peek(&cache_key1.to_string())
            .ok_or_else(|| color_eyre::eyre::eyre!("Texture 1 not found in cache: {}", cache_key1))?
            .texture;
        let texture_view1 = texture1.create_view(&wgpu::TextureViewDescriptor::default());

        let texture2 = &self
            .texture_cache
            .peek(&cache_key2.to_string())
            .ok_or_else(|| color_eyre::eyre::eyre!("Texture 2 not found in cache: {}", cache_key2))?
            .texture;
        let texture_view2 = texture2.create_view(&wgpu::TextureViewDescriptor::default());

        let transition_params = self
            .render_params_buf
            .as_ref()
            .expect("buffer initialized")
            .as_entire_buffer_binding();

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

        // Cache the bind group (conservative estimate of 256 bytes per bind group)
        const BIND_GROUP_SIZE_BYTES: u64 = 256;
        self.bind_group_cache
            .get_or_insert(bind_group_key.clone(), BIND_GROUP_SIZE_BYTES, || bind_group);
        Ok(bind_group_key)
    }

    /// Update render parameters for animations
    fn update_render_params(
        &mut self,
        progress: f32,
        anim_type: u32,
        direction: [f32; 2],
        target_size: (f32, f32),
        fit_mode: u32,
    ) -> color_eyre::Result<()> {
        let queue = self
            .queue
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Queue not initialized"))?;
        let render_params_buf = self
            .render_params_buf
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("Transition params buffer not initialized"))?;

        let render_params = RenderParams {
            progress,
            anim_type,
            fit_mode,
            _pad0: 0,
            direction,
            output_size: [target_size.0, target_size.1],
            background: DEFAULT_BACKGROUND,
        };

        queue.write_buffer(render_params_buf, 0, bytemuck::bytes_of(&render_params));
        Ok(())
    }

    /// Log cache metrics for monitoring and debugging
    fn log_cache_metrics(&self) {
        let tex_metrics = self.texture_cache.metrics();
        let bg_metrics = self.bind_group_cache.metrics();
        let frames = self.total_frames_rendered.load(Ordering::Relaxed);

        tracing::info!(
            texture_cache_size = tex_metrics.size,
            texture_cache_hits = tex_metrics.hits,
            texture_cache_misses = tex_metrics.misses,
            texture_hit_rate = format!("{:.1}%", tex_metrics.hit_rate()),
            texture_cache_mb = format!("{:.2}", tex_metrics.bytes_mb()),
            texture_evictions = tex_metrics.evictions,
            bind_group_cache_size = bg_metrics.size,
            bind_group_cache_hits = bg_metrics.hits,
            bind_group_cache_misses = bg_metrics.misses,
            bind_group_hit_rate = format!("{:.1}%", bg_metrics.hit_rate()),
            bind_group_cache_mb = format!("{:.2}", bg_metrics.bytes_mb()),
            bind_group_evictions = bg_metrics.evictions,
            total_textures_loaded = self.texture_cache.total_inserted(),
            total_frames_rendered = frames,
            "GPU cache metrics"
        );
    }

    /// Configure surface for a specific output
    #[tracing::instrument(skip(self),
        fields(output = output_name, width = size.0, height = size.1))]
    fn configure_surface(
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
        self.init_image_pipeline(surface_format)?;

        Ok(surface_format)
    }

    /// Unified rendering method that handles both transitions and instant switches.
    /// Refer to the arguments.
    ///
    /// # Arguments
    /// * `output_name` - Name of the output to render to
    /// * `previous_image` - Previous image for transition (None = use black dummy)
    /// * `current_image` - Current/target image to display
    /// * `progress` - Transition progress 0.0-1.0 (1.0 = show current fully, 0.0 = show previous)
    /// * `transition_type` - Type of transition effect (0 = crossfade, etc.)
    #[tracing::instrument(skip(self, previous_image, current_image),
        fields(output = output_name, progress = progress, transition_type = transition_type))]
    fn render_frame(
        &mut self,
        output_name: &str,
        previous_image: Option<&Path>,
        current_image: &Path,
        progress: f32,
        transition_type: u32,
        direction: Option<[f32; 2]>,
        fit_mode: u32,
    ) -> color_eyre::Result<()> {
        let direction = direction.unwrap_or([0.0, 0.0]);
        // Get target size
        let target_size = {
            let surface_config = self.surface_configs.get(output_name).ok_or_else(|| {
                color_eyre::eyre::eyre!("Surface config not found for output: {}", output_name)
            })?;
            (surface_config.width, surface_config.height)
        };

        // Load the current image texture
        let current_cache_key = Self::cache_key(current_image);
        self.load_image_texture(current_image, target_size)?;

        // Load or create previous texture
        let previous_cache_key = if let Some(prev_path) = previous_image {
            let key = Self::cache_key(prev_path);
            self.load_image_texture(prev_path, target_size)?;
            key
        } else {
            // Use black dummy for first render or when no previous image
            self.get_or_create_dummy_texture(target_size)?
        };

        // Create or get bind group with (previous, current) textures
        let bind_group_key =
            self.get_or_create_bind_group(&previous_cache_key, &current_cache_key)?;

        // Set render parameters
        self.update_render_params(
            progress,
            transition_type,
            direction,
            (target_size.0 as f32, target_size.1 as f32),
            fit_mode,
        )?;

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
        let _surface_acquire_timer = GpuOperationTimer::new("surface_acquire");
        let surface_texture = surface.get_current_texture()?;
        drop(_surface_acquire_timer);

        let surface_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let _encoder_timer = GpuOperationTimer::new("command_encoder");
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let _render_pass_timer = GpuOperationTimer::new("render_pass");
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
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
        drop(_encoder_timer);

        let _submit_timer = GpuOperationTimer::new("queue_submit");
        queue.submit(Some(encoder.finish()));
        drop(_submit_timer);

        let _present_timer = GpuOperationTimer::new("surface_present");
        surface_texture.present();
        drop(_present_timer);

        self.total_frames_rendered.fetch_add(1, Ordering::Relaxed);

        Ok(())
    }

    /// Get GPU metrics data for socket response
    fn get_metrics_data(&self) -> wayper_lib::socket::GpuMetricsData {
        let tex_metrics = self.texture_cache.metrics();
        let bg_metrics = self.bind_group_cache.metrics();

        wayper_lib::socket::GpuMetricsData {
            texture_cache_size: tex_metrics.size,
            texture_cache_hits: tex_metrics.hits,
            texture_cache_misses: tex_metrics.misses,
            bind_group_cache_size: bg_metrics.size,
            bind_group_cache_hits: bg_metrics.hits,
            bind_group_cache_misses: bg_metrics.misses,
            total_textures_loaded: self.texture_cache.total_inserted(),
            total_frames_rendered: self.total_frames_rendered.load(Ordering::Relaxed),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct RenderParams {
    progress: f32,
    anim_type: u32,
    fit_mode: u32,
    _pad0: u32,
    direction: [f32; 2],
    output_size: [f32; 2],
    background: [f32; 4],
}

struct TextureLoadRequest {
    image_path: PathBuf,
    output_name: String,
    max_2d: u32,
}

enum TextureLoadResult {
    Loaded {
        cache_key: String,
        image_data: image::RgbaImage,
        dimensions: (u32, u32),
    },
    Failed {
        cache_key: String,
    },
}

#[derive(strum::Display)]
pub enum RenderCommand {
    NewSurface {
        output_name: String,
        surface: wgpu::Surface<'static>,
    },

    ConfigureSurface {
        output_name: String,
        size: (u32, u32),
    },

    RequestTextureLoad {
        image_path: PathBuf,
        output_name: String,
    },

    RenderFrame {
        output_name: String,
        previous_image: Option<PathBuf>,
        current_image: PathBuf,
        progress: f32,
        transition_type: u32,
        direction: Option<[f32; 2]>,
        fit_mode: u32,
    },
    LogCacheMetrics,
    GetMetricsData {
        reply: oneshot::Sender<GpuMetricsData>,
    },
}

pub fn create_surface_from_handles(
    instance: &mut wgpu::Instance,
    display: *mut wayland_sys::client::wl_display,
    surfac: *mut wayland_sys::client::wl_proxy,
) -> Result<wgpu::Surface<'static>, color_eyre::eyre::Error> {
    let raw_display_handle = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
        NonNull::new(display as *mut _).unwrap(),
    ));
    let raw_window_handle = RawWindowHandle::Wayland(WaylandWindowHandle::new(
        NonNull::new(surfac as *mut _).unwrap(),
    ));
    let surface = unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle,
            raw_window_handle,
        })
    }?;
    Ok(surface)
}
