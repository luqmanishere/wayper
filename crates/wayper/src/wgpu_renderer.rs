use std::ptr::NonNull;

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
}

impl WgpuRenderer {
    pub fn new() -> Self {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
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

    pub fn init_image_pipeline(&mut self, surface_format: wgpu::TextureFormat) -> color_eyre::Result<()> {
        let device = self.device.as_ref().ok_or_else(|| color_eyre::eyre::eyre!("Device not initialized"))?;

        if self.image_pipeline.is_some() {
            return Ok(());
        }

        // Load shader
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Image Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../../shader.wgsl").into()),
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
            // Bottom-left: pos(-1, -1), uv(0, 1)
            -1.0, -1.0, 0.0, 1.0,
            // Bottom-right: pos(1, -1), uv(1, 1)
            1.0, -1.0, 1.0, 1.0,
            // Top-right: pos(1, 1), uv(1, 0)
            1.0, 1.0, 1.0, 0.0,
            // Top-left: pos(-1, 1), uv(0, 0)
            -1.0, 1.0, 0.0, 0.0,
        ];

        let indices = [0u16, 1, 2, 0, 2, 3];

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
        self.sampler = Some(sampler);

        println!("Image pipeline initialized successfully");
        Ok(())
    }
}
