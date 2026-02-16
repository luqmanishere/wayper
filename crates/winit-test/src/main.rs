use anyhow::{Context, Result, anyhow};
use std::{sync::mpsc, thread, time::{Duration, Instant}};
use ffmpeg_next as ffmpeg;

enum FfmpegCmd {
    Play,
    Pause,
    SeekSeconds(f64),
    Stop,
}

struct VideoFrame {
    rgba: Vec<u8>,
    pts_sec: f64,
}
use wgpu::Surface;
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalSize},
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

const SHADER: &str = r#"
struct VsOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
  // Fullscreen triangle
  var p = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -3.0),
    vec2<f32>( 3.0,  1.0),
    vec2<f32>(-1.0,  1.0)
  );
  var uv = array<vec2<f32>, 3>(
    vec2<f32>(0.0, 2.0),
    vec2<f32>(2.0, 0.0),
    vec2<f32>(0.0, 0.0)
  );

  var o: VsOut;
  o.pos = vec4<f32>(p[vid], 0.0, 1.0);
  o.uv  = uv[vid];
  return o;
}

@group(0) @binding(0) var video_tex: texture_2d<f32>;
@group(0) @binding(1) var video_samp: sampler;

@fragment
fn fs_main(i: VsOut) -> @location(0) vec4<f32> {
  let uv = clamp(i.uv, vec2<f32>(0.0), vec2<f32>(1.0));
  return textureSample(video_tex, video_samp, uv);
}
"#;

fn align_up(value: u32, align: u32) -> u32 {
    ((value + align - 1) / align) * align
}

fn probe_video_metadata(path: &str) -> Result<(u32, u32, f64)> {
    let ictx = ffmpeg::format::input(path).context("open input for probe")?;
    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| anyhow!("no video stream found"))?;
    let params = stream.parameters();
    let (w, h) = unsafe { ((*params.as_ptr()).width, (*params.as_ptr()).height) };
    if w <= 0 || h <= 0 {
        return Err(anyhow!("invalid video dimensions: {}x{}", w, h));
    }
    let avg = stream.avg_frame_rate();
    let r = stream.rate();
    let fps = if avg.denominator() != 0 && avg.numerator() != 0 {
        avg.numerator() as f64 / avg.denominator() as f64
    } else if r.denominator() != 0 && r.numerator() != 0 {
        r.numerator() as f64 / r.denominator() as f64
    } else {
        60.0
    };
    Ok((w as u32, h as u32, fps))
}

fn copy_rgba_tight(src: &ffmpeg::frame::Video, w: usize, h: usize, dst: &mut [u8]) -> Result<()> {
    let stride = src.stride(0);
    let data = src.data(0);
    let stride = stride as usize;
    let row_bytes = w * 4;

    if dst.len() != row_bytes * h {
        return Err(anyhow!("tight frame size mismatch"));
    }

    for y in 0..h {
        let src_row = &data[y * stride..y * stride + row_bytes];
        let dst_row = &mut dst[y * row_bytes..(y + 1) * row_bytes];
        dst_row.copy_from_slice(src_row);
    }
    Ok(())
}

fn pts_to_seconds(pts: i64, time_base: ffmpeg::Rational) -> f64 {
    let num = time_base.numerator() as f64;
    let den = time_base.denominator() as f64;
    pts as f64 * (num / den)
}

fn decode_video_rgba_realtime(
    path: &str,
    fps: f64,
    cmd_rx: mpsc::Receiver<FfmpegCmd>,
    tx: mpsc::SyncSender<VideoFrame>,
) -> Result<()> {
    let mut ictx = ffmpeg::format::input(path).context("open input for decode")?;
    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| anyhow!("no video stream found"))?;
    let stream_index = stream.index();
    let time_base = stream.time_base();
    let frame_step = if fps.is_finite() && fps > 0.0 {
        1.0 / fps
    } else {
        1.0 / 60.0
    };
    let mut next_pts_sec = 0.0;
    let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .context("video codec context")?;
    let mut decoder = context.decoder().video().context("video decoder")?;

    let width = decoder.width();
    let height = decoder.height();
    let mut scaler = ffmpeg::software::scaling::context::Context::get(
        decoder.format(),
        width,
        height,
        ffmpeg::format::Pixel::RGBA,
        width,
        height,
        ffmpeg::software::scaling::flag::Flags::BILINEAR,
    )
    .context("create scaler")?;

    let mut decoded = ffmpeg::frame::Video::empty();
    let mut rgba = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::RGBA, width, height);

    let mut playing = true;
    let mut packets = ictx.packets();

    'decode: loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                FfmpegCmd::Play => playing = true,
                FfmpegCmd::Pause => playing = false,
                FfmpegCmd::Stop => return Ok(()),
                FfmpegCmd::SeekSeconds(sec) => {
                    drop(packets);
                    let ts = (sec * ffmpeg::ffi::AV_TIME_BASE as f64) as i64;
                    ictx.seek(ts, ..).context("seek")?;
                    decoder.flush();
                    packets = ictx.packets();
                }
            }
        }

        if !playing {
            thread::sleep(Duration::from_millis(10));
            continue;
        }

        let next = packets.next();
        let Some((stream, packet)) = next else { break };
        if stream.index() != stream_index {
            continue;
        }

        decoder.send_packet(&packet).context("send packet")?;
        while decoder.receive_frame(&mut decoded).is_ok() {
            scaler.run(&decoded, &mut rgba).context("scale frame")?;
            let mut out = vec![0u8; (width as usize) * (height as usize) * 4];
            copy_rgba_tight(&rgba, width as usize, height as usize, &mut out)?;

            let pts_sec = decoded
                .pts()
                .or(decoded.timestamp())
                .map(|v| pts_to_seconds(v, time_base))
                .unwrap_or_else(|| {
                    let v = next_pts_sec;
                    next_pts_sec += frame_step;
                    v
                });
            if pts_sec >= next_pts_sec {
                next_pts_sec = pts_sec + frame_step;
            }

            let mut out = VideoFrame { rgba: out, pts_sec };
            loop {
                match tx.try_send(out) {
                    Ok(()) => break,
                    Err(mpsc::TrySendError::Disconnected(_)) => return Ok(()),
                    Err(mpsc::TrySendError::Full(o)) => {
                        out = o;
                        while let Ok(cmd) = cmd_rx.try_recv() {
                            match cmd {
                                FfmpegCmd::Play => playing = true,
                                FfmpegCmd::Pause => playing = false,
                                FfmpegCmd::Stop => return Ok(()),
                                FfmpegCmd::SeekSeconds(sec) => {
                                    drop(packets);
                                    let ts = (sec * ffmpeg::ffi::AV_TIME_BASE as f64) as i64;
                                    ictx.seek(ts, ..).context("seek")?;
                                    decoder.flush();
                                    packets = ictx.packets();
                                    next_pts_sec = 0.0;
                                    continue 'decode;
                                }
                            }
                        }
                        thread::sleep(Duration::from_millis(1));
                    }
                }
            }
        }
    }

    decoder.send_eof().context("send eof")?;
    while decoder.receive_frame(&mut decoded).is_ok() {
        scaler.run(&decoded, &mut rgba).context("scale frame")?;
        let mut out = vec![0u8; (width as usize) * (height as usize) * 4];
        copy_rgba_tight(&rgba, width as usize, height as usize, &mut out)?;

        let pts_sec = decoded
            .pts()
            .or(decoded.timestamp())
            .map(|v| pts_to_seconds(v, time_base))
            .unwrap_or_else(|| {
                let v = next_pts_sec;
                next_pts_sec += frame_step;
                v
            });
        if pts_sec >= next_pts_sec {
            next_pts_sec = pts_sec + frame_step;
        }

        if tx.send(VideoFrame { rgba: out, pts_sec }).is_err() {
            break;
        }
    }

    Ok(())
}

struct Gfx {
    // Surface is tied to the window lifetime in wgpu 0.20, but we store window + surface together,
    // and (carefully) extend the surface lifetime to 'static since window lives as long as Gfx.
    surface: Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,

    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,

    video_tex: wgpu::Texture,
    video_view: wgpu::TextureView,
    video_sampler: wgpu::Sampler,

    padded_bytes_per_row: u32,
    upload_buf: Vec<u8>,
    video_width: u32,
    video_height: u32,
}

impl Gfx {
    async fn new(window: &Window, video_w: u32, video_h: u32) -> Result<Self> {
        let size = window.inner_size();

        let instance = wgpu::Instance::default();

        // Create surface with real window lifetime...
        let surface_temp = instance.create_surface(window).context("create surface")?;
        // ...then extend to 'static (safe if the window outlives the surface, which it does here).
        let surface: Surface<'static> = unsafe { std::mem::transmute(surface_temp) };

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| anyhow!("no suitable GPU adapters found"))?;

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .context("request_device")?;

        let caps = surface.get_capabilities(&adapter);
        let surface_format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: caps.present_modes[0],
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // Video texture (RGBA). (For “real video” later, consider NV12 and convert in shader.)
        let video_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("video_tex"),
            size: wgpu::Extent3d {
                width: video_w,
                height: video_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let video_view = video_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let video_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("video_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bgl"),
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
            ],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&video_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&video_sampler),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pl"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        // wgpu row alignment: pad upload rows to 256-byte multiple
        let tight_bpr = video_w * 4;
        let padded_bpr = align_up(tight_bpr, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let upload_buf = vec![0u8; (padded_bpr as usize) * (video_h as usize)];

        Ok(Self {
            surface,
            device,
            queue,
            config,
            size,
            pipeline,
            bind_group,
            video_tex,
            video_view,
            video_sampler,
            padded_bytes_per_row: padded_bpr,
            upload_buf,
            video_width: video_w,
            video_height: video_h,
        })
    }

    fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }
        self.size = new_size;
        self.config.width = new_size.width;
        self.config.height = new_size.height;
        self.surface.configure(&self.device, &self.config);
    }

    fn upload_frame_rgba(&mut self, tight_frame: &[u8]) -> Result<()> {
        let w = self.video_width as usize;
        let h = self.video_height as usize;
        let tight_bpr = w * 4;
        let padded_bpr = self.padded_bytes_per_row as usize;

        if tight_frame.len() != tight_bpr * h {
            return Err(anyhow!("tight frame size mismatch"));
        }

        for y in 0..h {
            let src = &tight_frame[y * tight_bpr..(y + 1) * tight_bpr];
            let dst = &mut self.upload_buf[y * padded_bpr..y * padded_bpr + tight_bpr];
            dst.copy_from_slice(src);
        }

        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.video_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.upload_buf,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(self.padded_bytes_per_row),
                rows_per_image: Some(self.video_height),
            },
            wgpu::Extent3d {
                width: self.video_width,
                height: self.video_height,
                depth_or_array_layers: 1,
            },
        );

        Ok(())
    }

    fn render(&mut self) -> Result<()> {
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("encoder"),
            });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("renderpass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            rpass.set_pipeline(&self.pipeline);
            rpass.set_bind_group(0, &self.bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }
}

struct App {
    video_path: String,
    video_w: u32,
    video_h: u32,
    frame_interval: Duration,

    window: Option<Window>,
    gfx: Option<Gfx>,

    rx: Option<mpsc::Receiver<VideoFrame>>,
    pending_frame: Option<VideoFrame>,
    cmd_tx: Option<mpsc::Sender<FfmpegCmd>>,

    last_redraw: Instant,
    log_next_time: Instant,
    log_render_count: u32,
    log_new_frame_count: u32,
    start_time: Option<Instant>,
    base_pts: f64,
    log_pts_next_time: Instant,
    log_pts_count: u32,
    log_pts_accum: f64,
}

impl App {
    fn new(video_path: String, video_w: u32, video_h: u32, fps: f64) -> Self {
        let interval = if fps.is_finite() && fps > 0.0 {
            Duration::from_secs_f64(1.0 / fps)
        } else {
            Duration::from_millis(16)
        };
        let now = Instant::now();
        Self {
            video_path,
            video_w,
            video_h,
            frame_interval: interval,
            window: None,
            gfx: None,
            rx: None,
            pending_frame: None,
            cmd_tx: None,
            last_redraw: now,
            log_next_time: now + Duration::from_secs(1),
            log_render_count: 0,
            log_new_frame_count: 0,
            start_time: None,
            base_pts: 0.0,
            log_pts_next_time: now + Duration::from_secs(1),
            log_pts_count: 0,
            log_pts_accum: 0.0,
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attrs = Window::default_attributes()
            .with_title("winit 0.30 + wgpu 0.20 + ffmpeg rawvideo")
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = event_loop.create_window(attrs).expect("create window");

        // Start ffmpeg decode thread -> channel
        let (frame_tx, frame_rx) = mpsc::sync_channel::<VideoFrame>(1);
        let (cmd_tx, cmd_rx) = mpsc::channel::<FfmpegCmd>();
        let path = self.video_path.clone();
        let fps = 1.0 / self.frame_interval.as_secs_f64().max(0.000_1);

        thread::spawn(move || {
            let _ = decode_video_rgba_realtime(&path, fps, cmd_rx, frame_tx);
        });
        let _ = cmd_tx.send(FfmpegCmd::Play);

        let gfx =
            pollster::block_on(Gfx::new(&window, self.video_w, self.video_h)).expect("init wgpu");

        self.rx = Some(frame_rx);
        self.cmd_tx = Some(cmd_tx);
        self.gfx = Some(gfx);
        self.window = Some(window);

        self.last_redraw = Instant::now();
        self.log_next_time = self.last_redraw + Duration::from_secs(1);
        self.log_render_count = 0;
        self.log_new_frame_count = 0;
        self.start_time = None;
        self.base_pts = 0.0;
        self.log_pts_next_time = self.last_redraw + Duration::from_secs(1);
        self.log_pts_count = 0;
        self.log_pts_accum = 0.0;
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.last_redraw + self.frame_interval));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                if let Some(cmd_tx) = self.cmd_tx.as_ref() {
                    let _ = cmd_tx.send(FfmpegCmd::Stop);
                }
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(gfx) = self.gfx.as_mut() {
                    gfx.resize(size);
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                if self.pending_frame.is_none() {
                    if let Some(rx) = self.rx.as_ref() {
                        if let Ok(f) = rx.try_recv() {
                            self.pending_frame = Some(f);
                            self.log_new_frame_count = self.log_new_frame_count.saturating_add(1);
                        }
                    }
                }

                let Some(frame) = self.pending_frame.as_ref() else {
                    return;
                };

                let start = match self.start_time {
                    Some(s) => s,
                    None => {
                        self.start_time = Some(now);
                        self.base_pts = frame.pts_sec;
                        now
                    }
                };

                let target = start + Duration::from_secs_f64((frame.pts_sec - self.base_pts).max(0.0));
                if now < target {
                    event_loop.set_control_flow(ControlFlow::WaitUntil(target));
                    return;
                }

                self.log_pts_count = self.log_pts_count.saturating_add(1);
                self.log_pts_accum += frame.pts_sec;
                if now >= self.log_pts_next_time {
                    let avg_pts = if self.log_pts_count == 0 {
                        0.0
                    } else {
                        self.log_pts_accum / self.log_pts_count as f64
                    };
                    println!(
                        "pts avg: {:.3}s (count {}), base_pts {:.3}s",
                        avg_pts,
                        self.log_pts_count,
                        self.base_pts
                    );
                    self.log_pts_next_time = now + Duration::from_secs(1);
                    self.log_pts_count = 0;
                    self.log_pts_accum = 0.0;
                }

                self.last_redraw = now;
                self.log_render_count = self.log_render_count.saturating_add(1);
                if now >= self.log_next_time {
                    let elapsed = now.duration_since(self.log_next_time - Duration::from_secs(1));
                    let render_fps =
                        self.log_render_count as f64 / elapsed.as_secs_f64().max(0.000_1);
                    let new_fps =
                        self.log_new_frame_count as f64 / elapsed.as_secs_f64().max(0.000_1);
                    println!("render fps: {:.2}, new frames: {:.2}", render_fps, new_fps);
                    self.log_next_time = now + Duration::from_secs(1);
                    self.log_render_count = 0;
                    self.log_new_frame_count = 0;
                }

                if let (Some(gfx), Some(frame)) =
                    (self.gfx.as_mut(), self.pending_frame.take())
                {
                    let _ = gfx.upload_frame_rgba(&frame.rgba);
                    let _ = gfx.render();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = self.window.as_ref() {
            if let Some(frame) = self.pending_frame.as_ref() {
                let start = self.start_time.unwrap_or_else(Instant::now);
                let target =
                    start + Duration::from_secs_f64((frame.pts_sec - self.base_pts).max(0.0));
                if Instant::now() >= target {
                    window.request_redraw();
                }
                _event_loop.set_control_flow(ControlFlow::WaitUntil(target));
            } else {
                window.request_redraw();
                _event_loop
                    .set_control_flow(ControlFlow::WaitUntil(self.last_redraw + self.frame_interval));
            }
        }
    }
}

fn main() -> Result<()> {
    ffmpeg::init().context("init ffmpeg")?;

    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow!("Usage: cargo run --release -- <path-to-video>"))?;

    let (vw, vh, fps) = probe_video_metadata(&path)?;
    println!("Video dimensions: {}x{} @ {:.3} fps", vw, vh, fps);

    let event_loop = EventLoop::new().context("create event loop")?;
    let mut app = App::new(path, vw, vh, fps);
    event_loop.run_app(&mut app).context("run_app")?;

    Ok(())
}
