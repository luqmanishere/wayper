use std::{collections::HashMap, ptr::NonNull};

use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use smithay_client_toolkit::reexports::client::{
    Proxy,
    protocol::{
        wl_display::{self, WlDisplay},
        wl_surface::WlSurface,
    },
};
use wgpu::{hal::Queue, naga::FastHashMap};

pub struct WgpuRenderer {
    instance: wgpu::Instance,
    pub adapter: Option<wgpu::Adapter>,
    pub device: Option<wgpu::Device>,
    pub queue: Option<wgpu::Queue>,
    pub map: FastHashMap<String, wgpu::Surface<'static>>,
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
}
