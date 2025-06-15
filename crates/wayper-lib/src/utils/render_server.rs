use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, mpsc::Sender},
    thread::JoinHandle,
};

use image::{ImageBuffer, Rgba, RgbaImage};
use indexmap::IndexMap;
use tracing::Level;

pub type ImageJobMap = IndexMap<RenderJobRequest, RgbaImage>;

/// Server handling rendering jobs
#[derive(Debug)]
pub struct RenderServer {
    image_jobs: Arc<Mutex<ImageJobMap>>,
    jobs_requested: Arc<Mutex<std::collections::HashSet<RenderJobRequest>>>,
    jobs_sender_tx: Sender<RenderJobRequest>,
    _jobs_thread: JoinHandle<()>,
    job_wait: Arc<Condvar>,
}

impl RenderServer {
    /// Get a new copy of [`RenderServer`]. This call setups all data, conditions
    /// and threads the server requires. It is immediately ready to receive jobs when
    /// this method returns.
    pub fn new() -> Self {
        let (sender_tx, sender_rx) = std::sync::mpsc::channel::<RenderJobRequest>();
        let image_jobs = Arc::new(Mutex::new(IndexMap::default()));
        let jobs_requested = Arc::new(Mutex::new(HashSet::default()));
        let job_wait = Arc::new(Condvar::new());

        // handles for the thread
        let image_jobs_handle = Arc::clone(&image_jobs);
        let job_wait_handle = job_wait.clone();
        let jobs_requested_handle = Arc::clone(&jobs_requested);

        let th = std::thread::spawn(move || {
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
    pub fn get_job(&self, request: RenderJobRequest) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
        let mut lock = self.image_jobs.lock().unwrap();
        let mut manual_request = false;
        loop {
            match lock.get(&request) {
                Some(res) => {
                    tracing::info!("request completed");
                    return res.clone();
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

#[derive(Debug)]
pub enum RenderNotify {
    UpdateDim { width: u32, height: u32 },
    UpdateList {},
    SendMe,
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, strum::Display)]
pub enum RenderJobRequest {
    Image {
        width: u32,
        height: u32,
        image: PathBuf,
    },
    Die,
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
