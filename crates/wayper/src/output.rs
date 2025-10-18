//! Output. Data and processing happens here

use std::path::PathBuf;

use smithay_client_toolkit::{
    output::OutputInfo,
    reexports::{
        calloop,
        client::protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    },
    shell::wlr_layer::LayerSurface,
    shm::slot::Buffer,
};

use wayper_lib::config::OutputConfig;

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

    pub buffer: Option<Buffer>,
    pub _surface: Option<WlSurface>,
    pub _layer: LayerSurface,

    pub index: usize,
    pub img_list: Vec<PathBuf>,
    pub visible: bool,
    pub should_next: bool,
    pub last_render_instant: std::time::Instant,
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
