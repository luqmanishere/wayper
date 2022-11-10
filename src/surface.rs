use std::{
    cell::Cell,
    io::{BufWriter, Write},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::Instant,
};

use derivative::Derivative;
use eyre::{eyre, Context, Result};
use image::imageops::FilterType;
use rand::seq::SliceRandom;
use smithay_client_toolkit::{
    output::OutputInfo,
    reexports::{
        client::{
            protocol::{wl_buffer, wl_output, wl_shm, wl_surface},
            Attached, Main,
        },
        protocols::wlr::unstable::layer_shell::v1::client::{
            zwlr_layer_shell_v1, zwlr_layer_surface_v1,
        },
    },
    shm::AutoMemPool,
};
use tracing::{debug, info, trace};
use walkdir::WalkDir;

use crate::config::OutputConfig;

#[derive(PartialEq, Clone, Copy)]
enum RenderEvent {
    Configure { width: u32, height: u32 },
    Closed,
}

#[derive(Derivative)]
#[derivative(Debug)]
pub struct WallSurface {
    surface: wl_surface::WlSurface,
    layer_surface: Main<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    #[derivative(Debug = "ignore")]
    next_render_event: Rc<Cell<Option<RenderEvent>>>,
    pool: AutoMemPool,
    dimensions: (u32, u32),
    current_img: Option<PathBuf>,
    buffer: Option<wl_buffer::WlBuffer>,
    time_passed: Instant,
    redraw: bool,
    pub output_info: OutputInfo,
    pub output_config: Arc<OutputConfig>,
    img_list: Option<Vec<PathBuf>>,
    index: Option<usize>,
    hide: bool,
}

impl WallSurface {
    pub fn new(
        output: &wl_output::WlOutput,
        output_info: &OutputInfo,
        output_config: Arc<OutputConfig>,
        surface: wl_surface::WlSurface,
        layer_shell: &Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
        pool: AutoMemPool,
    ) -> Self {
        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            Some(output),
            zwlr_layer_shell_v1::Layer::Overlay,
            "wayper".to_owned(),
        );
        // set size to 0 so the compositor gives us the size
        layer_surface.set_size(0, 0);
        // input devices do not interact with us
        layer_surface
            .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
        // ignore exclusive zones and render on the entire screen
        layer_surface.set_exclusive_zone(-1);
        // anchor on all sides to cover the entire screen
        layer_surface.set_anchor(zwlr_layer_surface_v1::Anchor::all());
        // use the `background` layer, which puts us on the back
        layer_surface.set_layer(zwlr_layer_shell_v1::Layer::Background);

        // handle events
        let next_render_event = Rc::new(Cell::new(None::<RenderEvent>));
        let next_render_event_handle = Rc::clone(&next_render_event);
        layer_surface.quick_assign(move |layer_surface, event, _| {
            match (event, next_render_event_handle.get()) {
                (zwlr_layer_surface_v1::Event::Closed, _) => {
                    debug!("Got closed event!");
                    next_render_event_handle.set(Some(RenderEvent::Closed));
                }
                (
                    zwlr_layer_surface_v1::Event::Configure {
                        serial,
                        width,
                        height,
                    },
                    next,
                ) if next != Some(RenderEvent::Closed) => {
                    debug!("Got configure event");
                    layer_surface.ack_configure(serial);
                    debug!("sent ack_configure");
                    next_render_event_handle.set(Some(RenderEvent::Configure { width, height }));
                }
                (_, _) => {}
            }
        });

        // Commit so the server sends a configure event
        surface.commit();

        let img_list = if output_config.path.as_ref().unwrap().is_dir() {
            let mut files = WalkDir::new(output_config.path.as_ref().unwrap())
                .into_iter()
                .filter(|e| {
                    mime_guess::from_path(e.as_ref().unwrap().path())
                        .iter()
                        .any(|ev| ev.type_() == "image")
                })
                .map(|e| e.unwrap().path().to_owned())
                .collect::<Vec<_>>();

            let mut rng = rand::thread_rng();
            files.shuffle(&mut rng);
            dbg!(&files);
            Some(files)
        } else {
            None
        };

        let current_img = if output_config.path.as_ref().unwrap().is_file() {
            let path = &output_config.path;
            if mime_guess::from_path(path.as_ref().unwrap())
                .iter()
                .any(|ev| ev.type_() == "image")
            {
                path.clone()
            } else {
                None
            }
        } else {
            None
        };

        Self {
            surface,
            output_info: output_info.clone(),
            output_config,
            layer_surface,
            next_render_event,
            pool,
            dimensions: (0, 0),
            current_img,
            buffer: None,
            time_passed: Instant::now(),
            redraw: true,
            img_list,
            index: None,
            hide: false,
        }
    }

    /// Handle events that have occured since the last call
    /// returns true if the surface should be dropped
    pub fn handle_events(&mut self) -> bool {
        match self.next_render_event.take() {
            Some(RenderEvent::Closed) => true,
            Some(RenderEvent::Configure { width, height }) => {
                if self.dimensions != (width, height) {
                    self.dimensions = (width, height);
                }
                false
            }
            None => false,
        }
    }

    pub fn should_redraw(&mut self, time: &Instant) -> bool {
        let elapsed = {
            let delta = time.duration_since(self.time_passed);
            trace!("time since last update: {:?}", delta);
            std::time::Duration::from_secs(self.output_config.duration.unwrap() as u64)
                .saturating_sub(delta)
                == std::time::Duration::ZERO
        };

        (self.redraw || elapsed) && self.dimensions.0 != 0
    }

    pub fn next(&mut self) -> Option<&PathBuf> {
        if let Some(img_list) = &self.img_list {
            if let Some(index) = self.index {
                debug!("Current index is {}", index);
                if index < img_list.len() - 1 {
                    self.index = Some(index + 1);
                }
                if index == img_list.len() - 1 {
                    self.index = Some(0);
                }

                debug!("new index is {}", self.index.unwrap());
            } else {
                self.index = Some(0);
            }
            img_list.get(self.index.unwrap())
        } else {
            None
        }
    }

    /// the drawing function
    #[tracing::instrument(skip_all, fields(output = %self.output_info.name))]
    pub fn draw(&mut self, now: Instant) -> Result<()> {
        if self.dimensions == (0, 0) {
            return Err(eyre!(
                "dimensions are 0! has compositor sent a configure event?"
            ));
        }
        debug!(
            "dimensions are: {}, {}",
            self.dimensions.0, self.dimensions.1
        );
        let stride = 4 * self.dimensions.0 as i32;
        let width = self.dimensions.0 as i32;
        let height = self.dimensions.1 as i32;

        let path = if let Some(path) = self.next() {
            path.clone()
        } else {
            self.current_img
                .as_ref()
                .ok_or_else(|| eyre!("current_img is empty!"))?
                .to_path_buf()
        };

        let image = image::open(&path)?
            .resize_to_fill(
                width.try_into().unwrap(),
                height.try_into().unwrap(),
                FilterType::Lanczos3,
            )
            .into_rgba8();

        // Destroy current buffer if we have one
        if let Some(buffer) = &self.buffer {
            buffer.destroy();
        }

        // create new buffer, draw image to buffer
        if !self.hide {
            self.buffer = Some(
                self.pool
                    .try_draw::<_, eyre::Error>(
                        width,
                        height,
                        stride,
                        wl_shm::Format::Abgr8888,
                        |canvas: &mut [u8]| {
                            let mut writer = BufWriter::new(canvas);
                            writer.write_all(image.as_raw()).unwrap();
                            writer.flush().unwrap();
                            Ok(())
                        },
                    )
                    .context("creating wl_buffer in pool")?,
            );

            // Attach the buffer to the surface and mark the entire surface as damaged
            self.surface.attach(self.buffer.as_ref(), 0, 0);
            self.surface
                .damage_buffer(0, 0, width as i32, height as i32);

            // commit
            self.surface.commit();
            self.redraw = false;
            self.time_passed = now;
            info!("Finished drawing current wallpaper: {}", path.display());
        } else {
            self.buffer = Some(
                self.pool
                    .try_draw::<_, eyre::Error>(
                        width,
                        height,
                        stride,
                        wl_shm::Format::Abgr8888,
                        |canvas: &mut [u8]| {
                            canvas
                                .chunks_exact_mut(4)
                                .enumerate()
                                .for_each(|(index, chunk)| {
                                    let x = ((index) % width as usize) as i32;
                                    let y = (index / width as usize) as i32;

                                    let a = 0xFF;
                                    let r = i32::min(
                                        ((width - x) * 0xFF) / width,
                                        ((height - y) * 0xFF) / height,
                                    );
                                    let g = i32::min(
                                        (x * 0xFF) / width,
                                        ((height - y) * 0xFF) / height,
                                    );
                                    let b =
                                        i32::min(((width - x) * 0xFF) / width, (y * 0xFF) / height);
                                    let color = (a << 24) + (r << 16) + (g << 8) + b;

                                    let array: &mut [u8; 4] = chunk.try_into().unwrap();
                                    *array = color.to_le_bytes();
                                });
                            Ok(())
                        },
                    )
                    .context("creating wl_buffer in pool")?,
            );

            // Attach the buffer to the surface and mark the entire surface as damaged
            self.surface.attach(self.buffer.as_ref(), 0, 0);
            self.surface
                .damage_buffer(0, 0, width as i32, height as i32);

            // commit
            self.surface.commit();
            self.redraw = false;
            self.time_passed = now;
            info!("current wallpaper is hidden");
        }
        Ok(())
    }

    pub fn update_config(&mut self, output_config: Arc<OutputConfig>) -> Result<()> {
        debug!("Updating config");
        self.img_list = if output_config.path.as_ref().unwrap().is_dir() {
            debug!("Detected path as a directory");
            let mut files = WalkDir::new(output_config.path.as_ref().unwrap())
                .into_iter()
                .filter(|e| {
                    mime_guess::from_path(e.as_ref().unwrap().path())
                        .iter()
                        .any(|ev| ev.type_() == "image")
                })
                .map(|e| e.unwrap().path().to_owned())
                .collect::<Vec<_>>();

            let mut rng = rand::thread_rng();
            files.shuffle(&mut rng);
            dbg!(&files);
            Some(files)
        } else {
            None
        };

        self.current_img = if output_config.path.as_ref().unwrap().is_file() {
            debug!("Detected path as a file");
            let path = &output_config.path;
            if mime_guess::from_path(path.as_ref().unwrap())
                .iter()
                .any(|ev| ev.type_() == "image")
            {
                path.clone()
            } else {
                None
            }
        } else {
            None
        };
        self.output_config = output_config;
        self.redraw = true;
        Ok(())
    }

    pub fn current_img(&self) -> PathBuf {
        if let Some(path) = &self.current_img {
            path.clone()
        } else {
            self.img_list
                .as_ref()
                .unwrap()
                .get(self.index.unwrap())
                .unwrap()
                .clone()
        }
    }

    pub fn hide(&mut self) {
        self.hide = true;
    }

    pub fn show(&mut self) {
        self.hide = false;
    }

    pub fn toggle_visiblity(&mut self) {
        self.hide = !self.hide;
    }
}

impl Drop for WallSurface {
    fn drop(&mut self) {
        self.layer_surface.destroy();
        self.surface.destroy();
    }
}
