use std::sync::Arc;

use winit::window::Window;

pub struct Output {
    #[expect(unused)]
    pub name: String,
    pub size: (u32, u32),
    pub surface: wgpu::Surface<'static>,
    pub surface_format: wgpu::TextureFormat,
    pub window_arc: Arc<Window>,
}
