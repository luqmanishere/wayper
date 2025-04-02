//! Output data representation

use std::{
    io::{BufWriter, Write},
    path::PathBuf,
};

use smithay_client_toolkit::{
    output::OutputInfo,
    reexports::client::protocol::{wl_output::WlOutput, wl_shm, wl_surface::WlSurface},
    shell::{wlr_layer::LayerSurface, WaylandSurface},
    shm::slot::{Buffer, SlotPool},
};

use crate::config::OutputConfig;

// TODO: maybe all pub is not a good idea

#[derive(Debug)]
pub struct OutputRepr {
    pub output_name: String,
    #[allow(dead_code)]
    pub wl_repr: WlOutput,
    pub output_info: OutputInfo,
    pub output_config: Option<OutputConfig>,
    pub dimensions: Option<(u32, u32)>,
    #[allow(dead_code)]
    pub scale_factor: i64,
    pub first_configure: bool,

    pub pool: SlotPool,
    pub buffer: Option<Buffer>,
    pub surface: Option<WlSurface>,
    pub layer: LayerSurface,

    pub index: usize,
    pub img_list: Vec<PathBuf>,
    pub visible: bool,
}

impl OutputRepr {
    #[tracing::instrument(skip_all, fields(name=self.output_name))]
    pub fn update_config(&mut self, new_config: OutputConfig) {
        tracing::trace!("new config: {new_config:?}");
        if new_config
            .name
            .as_ref()
            .expect("config must have output name")
            == &self.output_name
        {
            self.output_config = Some(new_config);
            self.buffer = None;

            tracing::info!("received updated config");
        }
    }

    #[tracing::instrument(skip_all, fields(name=self.output_name))]
    pub fn draw(&mut self) -> color_eyre::Result<()> {
        if !self.visible {
            tracing::debug!("Not visible, not drawing");
            return Ok(());
        }

        tracing::trace!("begin drawing");
        let (width, height) = self.dimensions.expect("exists");
        let stride = width as i32 * 4;

        let path = self.next();
        tracing::info!("drawing: {}", path.display());

        let image = image::open(&path)?
            .resize_to_fill(width, height, image::imageops::FilterType::Lanczos3)
            .into_rgba8();

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
        self.layer.commit();

        // reuse the buffer created, since
        self.buffer = Some(buffer);

        if self.first_configure {
            self.first_configure = false;
        }
        tracing::trace!("finish drawing");
        Ok(())
    }

    /// if there is an image on current_img, give the image and increase index
    fn next(&mut self) -> PathBuf {
        let img_list = &self.img_list;
        let index = self.index;
        tracing::debug!("Current index is {}", index);
        if index < img_list.len() - 1 {
            self.index = index + 1;
        }
        if index == img_list.len() - 1 {
            self.index = 0;
        }

        tracing::debug!("new index is {}", self.index);
        img_list[self.index].clone()
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
