use std::sync::Arc;

use winit::window::Window;

pub struct Output {
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    pub surface_format: wgpu::TextureFormat,
    pub window_arc: Arc<Window>,
}
