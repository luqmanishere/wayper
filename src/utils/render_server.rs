use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{mpsc::Sender, Arc, Condvar, Mutex},
    thread::JoinHandle,
};

use clap::parser::Indices;
use derivative::Derivative;
use ffmpeg_next::software::scaling::Flags;
use image::{ImageBuffer, Rgba, RgbaImage};
use indexmap::IndexMap;
use mime::MSGPACK;
use tracing::Level;

pub type ImageJobMap = IndexMap<RenderJobRequest, RgbaImage>;
pub type VideoJobMap = IndexMap<RenderJobRequest, IndexMap<u32, ffmpeg_next::util::frame::Video>>;

/// Server handling rendering jobs
#[derive(Derivative)]
#[derivative(Debug)]
pub struct RenderServer {
    image_jobs: Arc<Mutex<ImageJobMap>>,
    jobs_requested: Arc<Mutex<std::collections::HashSet<RenderJobRequest>>>,
    jobs_sender_tx: Sender<RenderJobRequest>,
    _jobs_thread: JoinHandle<()>,
    job_wait: Arc<Condvar>,

    #[derivative(Debug = "ignore")]
    video_jobs: Arc<Mutex<VideoJobMap>>,
    video_job_wait: Arc<Condvar>,
}

impl RenderServer {
    /// Get a new copy of [`RenderServer`]. This call setups all data, conditions
    /// and threads the server requires. It is immediately ready to receive jobs when
    /// this method returns.
    pub fn new() -> Self {
        // initialize ffmpeg, whatever it does...
        ffmpeg_next::init().unwrap();

        let (sender_tx, sender_rx) = std::sync::mpsc::channel::<RenderJobRequest>();
        let image_jobs = Arc::new(Mutex::new(IndexMap::default()));
        let video_jobs = Arc::new(Mutex::new(IndexMap::default()));
        let jobs_requested = Arc::new(Mutex::new(HashSet::default()));
        let job_wait = Arc::new(Condvar::new());
        let video_wait = Arc::new(Condvar::new());

        // handles for the thread
        let image_jobs_handle = Arc::clone(&image_jobs);
        let job_wait_handle = job_wait.clone();
        let jobs_requested_handle = Arc::clone(&jobs_requested);
        let video_jobs_handle = video_jobs.clone();
        let video_wait_handle = video_wait.clone();

        let th = std::thread::spawn(move || {
            let mut video_render_context = IndexMap::new();
            loop {
                match sender_rx.recv() {
                    Ok(job) => {
                        let span = tracing::span!( Level::INFO, "render_server", job = ?job);
                        let _span = span.enter();

                        // TODO: better logging values
                        if job != RenderJobRequest::Die
                            && jobs_requested_handle.lock().unwrap().insert(job.clone())
                        {
                            tracing::event!(Level::INFO, "New job has been added");
                        } else {
                            tracing::event!(Level::INFO, "Old job requested, skipping...");
                            continue;
                        };
                        match &job {
                            RenderJobRequest::Die => break,
                            RenderJobRequest::Image {
                                width,
                                height,
                                image,
                            } => {
                                let _time = JobTime::new(span.clone());
                                let image = image::open(image)
                                    .expect("path exists")
                                    .resize_to_fill(
                                        *width,
                                        *height,
                                        image::imageops::FilterType::Lanczos3,
                                    )
                                    .into_rgba8();
                                image_jobs_handle.lock().unwrap().insert(job, image);
                                job_wait_handle.notify_all(); // not buffered so we can just call this
                            }
                            RenderJobRequest::Video {
                                frame_count,
                                video,
                                width,
                                height,
                            } => {
                                let ctx = if let Some(video_render_context) =
                                    video_render_context.get_mut(&job)
                                {
                                    video_render_context
                                } else {
                                    if let Ok(ictx) = ffmpeg_next::format::input(video) {
                                        let input = ictx
                                            .streams()
                                            .best(ffmpeg_next::media::Type::Video)
                                            .unwrap();
                                        let video_stream_index = input.index();

                                        let context_decoder =
                                            ffmpeg_next::codec::context::Context::from_parameters(
                                                input.parameters(),
                                            )
                                            .unwrap();
                                        let mut decoder =
                                            context_decoder.decoder().video().unwrap();
                                        decoder.set_frame_rate(Some(30.0));

                                        let scaler =
                                            ffmpeg_next::software::scaling::context::Context::get(
                                                decoder.format(),
                                                decoder.width(),
                                                decoder.height(),
                                                ffmpeg_next::format::Pixel::ARGB,
                                                *width,
                                                *height,
                                                Flags::BILINEAR,
                                            )
                                            .unwrap();

                                        let ctx = VideoRenderContext {
                                            path: video.clone(),
                                            ictx,
                                            scaler,
                                            decoder,
                                            video_stream_index,
                                        };
                                        video_render_context.insert(job.clone(), ctx);
                                        video_render_context.get_mut(&job).unwrap()
                                    } else {
                                        panic!("ffmpeg problems");
                                    }
                                };

                                // TODO: figure out how much to render
                                let mut frame_index = 0;

                                let instant = std::time::Instant::now();
                                let mut map = IndexMap::default();
                                for (stream, packet) in ctx.ictx.packets() {
                                    if stream.index() == ctx.video_stream_index {
                                        ctx.decoder.send_packet(&packet).unwrap();
                                        receive_and_process_decoded_frames(
                                            &mut ctx.decoder,
                                            &mut frame_index,
                                            &mut ctx.scaler,
                                            &mut map,
                                        )
                                        .unwrap();
                                    }
                                }
                                ctx.decoder.send_eof().unwrap();
                                receive_and_process_decoded_frames(
                                    &mut ctx.decoder,
                                    &mut frame_index,
                                    &mut ctx.scaler,
                                    &mut map,
                                )
                                .unwrap();
                                tracing::info!(
                                    "video finished in {}s ()",
                                    instant.elapsed().as_secs_f32()
                                );
                                video_jobs_handle.lock().unwrap().insert(job.clone(), map);
                                video_wait_handle.notify_all();
                            }
                        }
                    }
                    Err(e) => {
                        panic!("sending channel should not be closed? {e}");
                    }
                }
                // cleanup after a limit
                if image_jobs_handle.lock().unwrap().len() > 5 {
                    let mut map = image_jobs_handle.lock().unwrap();
                    let job_key = {
                        let (k, _) = map.get_index_mut(0).unwrap();
                        k.clone()
                    };
                    tracing::info!("Cleaning up {job_key:?}");
                    map.shift_remove(&job_key);
                    jobs_requested_handle.lock().unwrap().remove(&job_key);
                }
            }
            tracing::warn!("render server stopping");
        });

        Self {
            image_jobs,
            jobs_requested,
            jobs_sender_tx: sender_tx,
            _jobs_thread: th,
            job_wait,
            video_jobs,
            video_job_wait: video_wait,
        }
    }

    /// Get a cloned copy of the [`Sender`]
    pub fn get_new_tx(&self) -> Sender<RenderJobRequest> {
        self.jobs_sender_tx.clone()
    }

    /// Submit a new job to the server
    pub fn submit_job(&self, request: RenderJobRequest) -> color_eyre::Result<()> {
        Ok(self.jobs_sender_tx.send(request)?)
    }

    /// Get the requested job, if not yet requested then the job is added to the queue
    pub fn get_job(&self, request: RenderJobRequest) -> RenderJobResult {
        match &request {
            RenderJobRequest::Image { .. } => {
                let mut lock = self.image_jobs.lock().unwrap();
                let mut manual_request = false;
                loop {
                    match lock.get(&request) {
                        Some(res) => {
                            tracing::info!("request completed");
                            return RenderJobResult::Image(res.clone());
                        }
                        None => {
                            if self.jobs_requested.lock().unwrap().contains(&request) {
                                lock = self.job_wait.wait(lock).unwrap();
                            } else if !manual_request {
                                self.jobs_sender_tx.send(request.clone()).unwrap();
                                manual_request = true;
                            } else {
                                lock = self.job_wait.wait(lock).unwrap();
                            }
                        }
                    }
                }
            }
            RenderJobRequest::Video {
                width,
                height,
                video,
                frame_count,
            } => {
                let mut lock = self.video_jobs.lock().unwrap();
                loop {
                    match lock.get(&request) {
                        Some(map) => match map.get(frame_count) {
                            Some(res) => {
                                return RenderJobResult::VideoFrame {
                                    frame_number: *frame_count,
                                    frame: res.clone(),
                                };
                            }
                            None => {
                                lock = self.video_job_wait.wait(lock).unwrap();
                            }
                        },
                        None => {
                            lock = self.video_job_wait.wait(lock).unwrap();
                        }
                    };
                }
            }
            RenderJobRequest::Die => {
                panic!()
            }
        }
    }
}

impl Default for RenderServer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RenderServer {
    fn drop(&mut self) {
        self.submit_job(RenderJobRequest::Die).unwrap();
    }
}

#[derive(Debug, Eq, Clone, strum::Display, Derivative)]
#[derivative(Hash, PartialEq)]
pub enum RenderJobRequest {
    Image {
        width: u32,
        height: u32,
        image: PathBuf,
    },
    Video {
        width: u32,
        height: u32,
        video: PathBuf,
        #[derivative(PartialEq = "ignore")]
        #[derivative(Hash = "ignore")]
        frame_count: u32,
    },
    Die,
}

/*
impl PartialEq for RenderJobRequest {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                RenderJobRequest::Image {
                    width: width1,
                    height: height1,
                    image: image1,
                },
                RenderJobRequest::Image {
                    width: width2,
                    height: height2,
                    image: image2,
                },
            ) => width1 == width2 && height1 == height2 && image1 == image2,
            (
                RenderJobRequest::Video {
                    width: width1,
                    height: height1,
                    video: video1,
                    ..
                },
                RenderJobRequest::Video {
                    width: width2,
                    height: height2,
                    video: video2,
                    ..
                },
            ) => width1 == width2 && height1 == height2 && video1 == video2,
            (RenderJobRequest::Die, RenderJobRequest::Die) => true,
            _ => false,
        }
    }
}

impl std::hash::Hash for RenderJobRequest {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            RenderJobRequest::Image {
                width,
                height,
                image,
            } => {
                width.hash(state);
                height.hash(state);
                image.hash(state);
            }
            RenderJobRequest::Video {
                width,
                height,
                video,
                ..
            } => {
                width.hash(state);
                height.hash(state);
                video.hash(state);
            }
            RenderJobRequest::Die => self.hash(state),
        }
    }
} */

pub enum RenderJobResult {
    Image(RgbaImage),
    VideoData {},
    VideoFrame {
        frame_number: u32,
        frame: ffmpeg_next::util::frame::Video,
    },
}

pub struct VideoFrame {
    frame_number: u32,
    frame_rate: u32,
    frame: ffmpeg_next::util::frame::Video,
}

#[derive(derivative::Derivative)]
#[derivative(Debug)]
pub struct VideoRenderContext {
    path: PathBuf,
    #[derivative(Debug = "ignore")]
    ictx: ffmpeg_next::format::context::Input,
    #[derivative(Debug = "ignore")]
    scaler: ffmpeg_next::software::scaling::Context,
    #[derivative(Debug = "ignore")]
    decoder: ffmpeg_next::decoder::Video,
    /// index of stream that is the video stream
    video_stream_index: usize,
}

pub struct JobTime {
    start: std::time::Instant,
    span: tracing::Span,
}
impl JobTime {
    pub fn new(span: tracing::Span) -> Self {
        Self {
            start: std::time::Instant::now(),
            span,
        }
    }
}

impl Drop for JobTime {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed().as_millis();
        self.span
            .in_scope(|| tracing::info!("job time elapsed: {elapsed}ms"));
    }
}

fn receive_and_process_decoded_frames(
    decoder: &mut ffmpeg_next::decoder::Video,
    frame_index: &mut u32,
    scaler: &mut ffmpeg_next::software::scaling::Context,
    frames_map: &mut IndexMap<u32, ffmpeg_next::frame::Video>,
) -> Result<(), ffmpeg_next::Error> {
    use ffmpeg_next::util::frame::video::Video;
    let mut decoded = Video::empty();
    while decoder.receive_frame(&mut decoded).is_ok() {
        let mut rgb_frame = Video::empty();
        scaler.run(&decoded, &mut rgb_frame)?;
        // save_file(&rgb_frame, frame_index).unwrap();
        frames_map.insert(*frame_index, rgb_frame);
        *frame_index += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_render() {
        let server = RenderServer::default();
        // server.submit_job(RenderJobRequest::Video {}).unwrap();
        // server.get_job(RenderJobRequest::Video {});
        // TODO: remove
        std::thread::sleep(std::time::Duration::from_secs(180));
    }
}
