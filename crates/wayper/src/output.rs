//! Output. Data and processing happens here

use std::{
    io::{BufWriter, Write},
    path::PathBuf,
    sync::Arc,
};

use color_eyre::eyre::Context;
use smithay_client_toolkit::{
    output::OutputInfo,
    reexports::{
        calloop,
        client::protocol::{wl_output::WlOutput, wl_shm, wl_surface::WlSurface},
    },
    shell::{WaylandSurface, wlr_layer::LayerSurface},
    shm::slot::{Buffer, SlotPool},
};

use wayper_lib::config::OutputConfig;

use crate::render_server::{RenderJobRequest, RenderServer};

// TODO: maybe all pub is not a good idea

#[derive(Debug)]
pub struct OutputRepr {
    pub output_name: String,
    pub _wl_repr: WlOutput,
    pub output_info: OutputInfo,
    pub output_config: Option<OutputConfig>,
    pub dimensions: Option<(u32, u32)>,
    pub _scale_factor: i64,
    pub first_configure: bool,
    /// Use to fire an instant draw command
    pub ping_draw: Option<calloop::ping::Ping>,

    pub pool: SlotPool,
    pub buffer: Option<Buffer>,
    pub _surface: Option<WlSurface>,
    pub layer: LayerSurface,

    pub index: usize,
    pub img_list: Vec<PathBuf>,
    pub visible: bool,
    pub should_next: bool,
    pub last_render_instant: std::time::Instant,
    pub render_server: Arc<RenderServer>,
}

impl OutputRepr {
    #[tracing::instrument(skip_all, fields(name=self.output_name))]
    #[expect(unused)]
    pub fn update_config(&mut self, new_config: OutputConfig) {
        // TODO: implement daemon config update
        tracing::trace!("new config: {new_config:?}");
        // if new_config
        //     .name
        //     .as_ref()
        //     .expect("config must have output name")
        //     == &self.output_name
        // {
        self.output_config = Some(new_config);
        self.buffer = None;

        tracing::info!("received updated config");
        // }
    }

    /// Returns the rendered path
    #[tracing::instrument(skip_all, fields(name=self.output_name))]
    pub fn draw(&mut self) -> color_eyre::Result<PathBuf> {
        let instant = std::time::Instant::now();
        if !self.visible {
            tracing::debug!("Not visible, not drawing");
            return Ok(Default::default());
        }

        tracing::trace!("begin drawing");
        let (width, height) = self.dimensions.expect("exists");
        let stride = width as i32 * 4;

        let path = self.next();
        tracing::info!("drawing: {}", path.display());

        let request = RenderJobRequest::Image {
            width,
            height,
            image: path.clone(),
        };

        // only the first render will be synchronous with the request. subsequent renders are queued
        let image = self.render_server.get_job(request);

        // check if buffer exists
        let (buffer, canvas) = if let Some(buffer) = self.buffer.take() {
            match self.pool.canvas(&buffer) {
                Some(canvas) => (buffer, canvas),
                None => {
                    tracing::warn!("Missing canvas when buffer exists!");
                    let (buffer, canvas) = self
                        .pool
                        .create_buffer(
                            width as i32,
                            height as i32,
                            stride,
                            wl_shm::Format::Abgr8888,
                        )
                        .expect("create buffer");
                    (buffer, canvas)
                }
            }
        } else {
            let (buffer, canvas) = self
                .pool
                .create_buffer(
                    width as i32,
                    height as i32,
                    stride,
                    wl_shm::Format::Abgr8888,
                )
                .expect("create buffer");
            (buffer, canvas)
        };

        // Draw to the window:
        {
            let mut writer = BufWriter::new(canvas);
            writer.write_all(image.as_raw()).unwrap();
            writer.flush().unwrap();
        }

        // Damage the entire window
        self.layer
            .wl_surface()
            .damage_buffer(0, 0, width as i32, height as i32);

        // Attach and commit to present.
        buffer
            .attach_to(self.layer.wl_surface())
            .expect("buffer attach");
        self.layer.wl_surface().commit();

        // reuse the buffer created, since
        self.buffer = Some(buffer);

        if self.first_configure {
            self.first_configure = false;
        }
        tracing::trace!("finish drawing");

        self.render_server
            .submit_job(RenderJobRequest::Image {
                width,
                height,
                image: self.peek_next_img(),
            })
            .wrap_err("Error sending job to render server")?;

        tracing::info!("draw elapsed time: {}ms", instant.elapsed().as_millis());
        Ok(path)
    }

    /// Increment the index and give the image. If its the first configure, it uses
    /// an index of 0
    pub fn next(&mut self) -> PathBuf {
        let img_list = &self.img_list;
        tracing::debug!("Current index is {}", self.index);
        self.index = self.get_next_index();

        tracing::debug!("new index is {}", self.index);
        img_list[self.index].clone()
    }

    /// get the next image, without incrementing the index
    pub fn peek_next_img(&self) -> PathBuf {
        self.img_list[self.get_next_index()].clone()
    }

    /// Get the next index, but does not modify the original index. Accounts
    /// for the length of the image vec
    fn get_next_index(&self) -> usize {
        let mut index = self.index;

        // the first render should use the first entry
        if self.first_configure {
            return 0;
        }

        // compute the next index
        match index.cmp(&(self.img_list.len() - 1)) {
            std::cmp::Ordering::Less => index += 1,
            std::cmp::Ordering::Equal => index = 0,
            std::cmp::Ordering::Greater => {
                panic!("index cannot be greated than the reference buffer")
            }
        }
        index
    }

    /// Gives the current image, if any
    pub fn current_img(&self) -> Option<PathBuf> {
        self.img_list.get(self.index).cloned()
    }

    /// Toggle the visibility state
    pub fn toggle_visible(&mut self) {
        self.visible = !self.visible;
        // TODO: rerender
    }
}
