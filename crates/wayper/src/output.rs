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

use wayper_lib::config::{OutputConfig, TransitionType};

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
    pub transition: Option<TransitionData>,

    /// When this output was created/added
    pub created_at: std::time::Instant,
    /// Total number of frames rendered for this output
    pub frame_count: u64,
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

    /// Get the previous image for transitions.
    /// Returns None on first render (based on self.first_configure).
    /// Otherwise returns the image at the previous index (with wrapping).
    pub fn previous_img(&self) -> Option<PathBuf> {
        if self.first_configure {
            return None;
        }

        let prev_index = if self.index == 0 {
            // Wrap around: if currently at 0, previous is last image
            self.img_list.len() - 1
        } else {
            self.index - 1
        };

        self.img_list.get(prev_index).cloned()
    }

    /// Toggle the visibility state
    pub fn toggle_visible(&mut self) {
        self.visible = !self.visible;
        // TODO: rerender
    }
}

#[derive(Debug)]
pub struct TransitionData {
    pub transition_type: TransitionType,
    pub start_time: Option<std::time::Instant>,
    pub duration_ms: u32,
    pub target_fps: u16,
    pub last_frame_time: std::time::Instant,
    pub direction: [f32; 2],
}

impl TransitionData {
    /// Create a new transition with the given parameters
    /// Timer starts on first frame render, not at creation time
    pub fn new(
        transition_type: TransitionType,
        duration_ms: u32,
        target_fps: u16,
        direction: [f32; 2],
    ) -> Self {
        Self {
            transition_type,
            start_time: None, // Will be set on first frame
            duration_ms,
            target_fps,
            last_frame_time: std::time::Instant::now(),
            direction,
        }
    }

    /// Start the transition timer (called on first frame)
    pub fn start(&mut self) {
        if self.start_time.is_none() {
            self.start_time = Some(std::time::Instant::now());
        }
    }

    /// Calculate current progress (0.0 to 1.0) based on elapsed time
    pub fn progress(&self) -> f32 {
        let Some(start_time) = self.start_time else {
            return 0.0; // Not started yet
        };
        let elapsed = start_time.elapsed().as_millis() as f32;
        let duration = self.duration_ms as f32;
        (elapsed / duration).min(1.0) // Clamp to 1.0
    }

    /// Calculate eased progress for smoother animations (ease in-out cubic)
    pub fn eased_progress(&self) -> f32 {
        let t = self.progress();
        if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
        }
    }

    /// Check if animation is complete
    pub fn is_complete(&self) -> bool {
        let Some(start_time) = self.start_time else {
            return false; // Not started yet
        };
        start_time.elapsed().as_millis() >= self.duration_ms as u128
    }

    /// Check if we should render a new frame based on target FPS
    pub fn should_render_frame(&mut self) -> bool {
        let target_frame_time = 1000.0 / self.target_fps as f32;
        let elapsed = self.last_frame_time.elapsed().as_millis() as f32;

        if elapsed >= target_frame_time {
            self.last_frame_time = std::time::Instant::now();
            true
        } else {
            false
        }
    }
}
