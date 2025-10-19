//! Code for the socket daemon and client communication

use std::{
    io::{BufRead, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::mpsc::SyncSender as StdSender,
    thread::JoinHandle,
};

use calloop::channel::Sender;
use clap::Subcommand;
use clap_complete::{ArgValueCandidates, CompletionCandidate};
use color_eyre::eyre::{WrapErr, eyre};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Helper struct for socket management
pub struct WayperSocket {
    /// The path to the socket
    pub socket_path: PathBuf,
    pub tx: Sender<(SocketCommand, StdSender<SocketOutput>)>,
    spawned: bool,
    sender_thread_handle: Option<JoinHandle<Result<(), SocketError>>>,
}

impl WayperSocket {
    /// Creates a new instance of the socket.
    pub fn new(socket_path: PathBuf, tx: Sender<(SocketCommand, StdSender<SocketOutput>)>) -> Self {
        Self {
            socket_path,
            tx,
            spawned: false,
            sender_thread_handle: None,
        }
    }

    /// Spawns a thread to forward accepted connections for the socket.
    /// Returns an Error if called more than once.
    pub fn socket_sender_thread(&mut self) -> Result<(), SocketError> {
        let socket_path = self.socket_path.clone();
        let socket_tx = self.tx.clone();

        if !self.spawned {
            self.spawned = true;
            let sender_thread = std::thread::spawn(move || -> Result<(), SocketError> {
                // metadata returns an [`Ok`] if the path exists
                if std::fs::metadata(&socket_path).is_ok() {
                    tracing::info!(
                        "previous socket detected at \"{}\". removing!",
                        socket_path.display()
                    );
                    std::fs::remove_file(&socket_path).map_err(|error| {
                        SocketError::CannotDeletePreviousSocket {
                            socket_path: socket_path.clone(),
                            error: error.to_string(),
                        }
                    })?;
                }

                // Create a new [`UnixListener`] by binding to the socket path
                let unix_listener = UnixListener::bind(&socket_path).map_err(|e| {
                    SocketError::CannotBindUnixSocket {
                        socket_path,
                        error: e.to_string(),
                    }
                })?;

                loop {
                    // loop, wait for connections, accept and handle
                    match unix_listener.accept() {
                        Ok((mut unix_stream, remote_addr)) => {
                            tracing::info!("received socket connection from {remote_addr:?}",);
                            let socket_tx = socket_tx.clone();

                            std::thread::spawn(move || {
                                tracing::info!("spawned to handle stream");

                                while let Ok(socket_output) =
                                    SocketCommand::from_socket(&mut unix_stream)
                                {
                                    let (reply_tx, reply_rx) =
                                        std::sync::mpsc::sync_channel::<SocketOutput>(1);
                                    socket_tx.send((socket_output, reply_tx)).unwrap();

                                    while let Ok(res) = reply_rx.recv() {
                                        match res.write_to_socket(&mut unix_stream) {
                                            Ok(_) => {}
                                            Err(err) => {
                                                tracing::error!("error writing to socket: {}", err)
                                            }
                                        }
                                    }
                                }

                                tracing::info!("stream fully handled");
                            });
                        }
                        Err(e) => {
                            tracing::error!("failed accepting connection from unixlistener: {e}");
                            continue;
                        }
                    }
                }
            });

            self.sender_thread_handle = Some(sender_thread);
            Ok(())
        } else {
            Err(SocketError::SpawnSenderOnce { socket_path })
        }
    }
}

/// List of commands supported by the socket. These commands and their arguments implement
/// serialization methods to be sent across the socket.
#[derive(
    Subcommand, Deserialize, Serialize, strum::Display, strum::EnumString, strum::VariantNames,
)]
#[strum(serialize_all = "kebab-case")]
pub enum SocketCommand {
    /// Ping the daemon, check if it's alive.
    Ping,
    /// Gets the current wallpaper
    Current {
        #[arg(short, long)]
        output_name: Option<String>,
    },
    /// Toggles showing the wallpaper for outputs.
    ///
    /// If no output name is specified, toggles all outputs, regardless of visibility.
    Toggle {
        #[arg(short, long)]
        output_name: Option<String>,
    },
    Hide {
        #[arg(short, long)]
        output_name: Option<String>,
    },
    Show {
        #[arg(short, long)]
        output_name: Option<String>,
    },
    /// Change profile to the specified one or default
    ChangeProfile {
        /// If unspecified, the default specified in the config will be used.
        #[arg(add = ArgValueCandidates::new(|| profiles_from_socket_or_config()))]
        profile_name: Option<String>,
    },

    /// Display a list of configured profiles
    Profiles,

    /// Display GPU performance metrics
    GpuMetrics,
}

fn profiles_from_socket_or_config() -> Vec<CompletionCandidate> {
    const SOCKET_PATH: &str = "/tmp/wayper/.socket.sock";
    let path = std::path::Path::new(SOCKET_PATH);
    if path.exists()
        && let Ok(mut stream) = UnixStream::connect(path)
        && let Ok(_) = SocketCommand::Profiles.write_to_socket(&mut stream)
        && let Ok(replies) = SocketOutput::from_socket(&mut stream)
        && let Some(SocketOutput::Profiles(profiles)) = replies.first()
    {
        profiles.iter().map(CompletionCandidate::new).collect()
    } else {
        vec![]
    }
}

impl SocketCommand {
    pub fn from_socket(stream: &mut UnixStream) -> color_eyre::Result<Self> {
        let mut msg = String::new();
        let mut reader = std::io::BufReader::new(stream);
        let read = reader
            .read_line(&mut msg)
            .context("failed to read the stream")?;

        // err if closed
        if read == 0 {
            return Err(eyre!("socket closed!"));
        }

        tracing::debug!("message received on socket: {msg}");

        // Type inference via return. Rust is pretty cool
        Ok(serde_json::from_str(&msg)?)
    }

    /// Serialize and write the command to the socket.
    pub fn write_to_socket(&self, stream: &mut UnixStream) -> color_eyre::Result<()> {
        let s = serde_json::to_string(self)?;
        write_to_stream(stream, s)?;
        Ok(())
    }
}

#[derive(Error, Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum SocketError {
    #[error("No current image for the output: {output}")]
    NoCurrentImage { output: String },
    #[error("Unidentified output provided: {output_name}")]
    UnindentifiedOutput { output_name: String },
    #[error("Unexpected error occured.")]
    UnexpectedError,
    #[error("Profile \"{0}\" is not defined.")]
    NoProfile(String),

    #[error("Daemon unimplemented command: {command}")]
    CommandUnimplemented { command: String },

    // process errros
    #[error("Unable to delete the previous socket at {socket_path}: {error}")]
    CannotDeletePreviousSocket { socket_path: PathBuf, error: String },
    #[error("Unable to bind to socket at {socket_path}: {error}")]
    CannotBindUnixSocket { socket_path: PathBuf, error: String },
    #[error(
        "The accepted socket connection sender thread should only be called once. Socket: {socket_path}"
    )]
    SpawnSenderOnce { socket_path: PathBuf },
}

impl SocketError {
    pub fn write_to_socket(&self, stream: &mut UnixStream) -> color_eyre::Result<()> {
        SocketOutput::SingleError(self.clone()).write_to_socket(stream)
    }
}

/// Wrapper type over possible responses from the daemon. These implement serialization to
/// be sent across the socket
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum SocketOutput {
    /// A string message
    Message(String),
    /// The current wallpaper
    CurrentWallpaper(OutputWallpaper),
    /// List of wallpapers currently loaded
    Wallpapers(Vec<OutputWallpaper>),
    SingleError(SocketError),
    MultipleErrors(Vec<SocketError>),
    Profiles(Vec<String>),
    /// GPU performance metrics
    GpuMetrics(GpuMetricsData),
    /// Signals end of reply for the previous request.
    End(String),
}

impl SocketOutput {
    /// Converts the output to JSON
    pub fn to_json(&self) -> color_eyre::Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    /// Parses the provided JSON into this format.
    pub fn from_json(s: &str) -> color_eyre::Result<Self> {
        Ok(serde_json::from_str(s)?)
    }

    /// Write the output to the stream.
    pub fn write_to_socket(&self, stream: &mut UnixStream) -> color_eyre::Result<()> {
        let s = self.to_json()?;
        tracing::debug!("replying to socket with response: {s}");
        write_to_stream(stream, s)?;
        Ok(())
    }

    /// Gets the output from the socket stream, parse it into [`Self`]
    pub fn from_socket(stream: &mut UnixStream) -> color_eyre::Result<Vec<Self>> {
        let mut reader = std::io::BufReader::new(stream);
        let mut response = String::new();
        let mut vec = vec![];

        loop {
            response.clear();
            let read = reader.read_line(&mut response)?;

            // TODO: trim?

            if read == 0 {
                return Err(eyre!("socket closed"));
            }

            tracing::debug!("socket replied with response: {response}");

            let output = Self::from_json(&response)?;
            if let SocketOutput::End(_) = output {
                break;
            } else {
                vec.push(output);
            }
        }
        Ok(vec)
    }
}

impl From<SocketError> for SocketOutput {
    fn from(value: SocketError) -> Self {
        SocketOutput::SingleError(value)
    }
}

impl std::fmt::Display for SocketOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            SocketOutput::Message(msg) => msg.to_string(),
            SocketOutput::CurrentWallpaper(output_wallpaper) => output_wallpaper.to_string(),
            SocketOutput::Wallpapers(output_wallpapers) => output_wallpapers
                .iter()
                .map(|output_wallpaper| {
                    format!(
                        "{}: {}",
                        output_wallpaper.output_name, output_wallpaper.wallpaper
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
            SocketOutput::SingleError(socket_error) => socket_error.to_string(),
            SocketOutput::MultipleErrors(socket_errors) => socket_errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
            SocketOutput::Profiles(items) => items.join("\n"),
            SocketOutput::GpuMetrics(metrics) => metrics.to_string(),
            SocketOutput::End(command) => format!("end of command {command}"),
        };

        write!(f, "{text}")
    }
}

/// Struct for wallpaper info from the daemon
#[derive(Deserialize, Serialize, Debug, PartialEq, Eq)]
pub struct OutputWallpaper {
    /// The name of the output
    pub output_name: String,
    /// The path to the wallpaper
    pub wallpaper: String,
}

impl std::fmt::Display for OutputWallpaper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.output_name, self.wallpaper)
    }
}

/// GPU performance metrics data
#[derive(Deserialize, Serialize, Debug, PartialEq, Eq, Clone)]
pub struct GpuMetricsData {
    pub texture_cache_size: usize,
    pub texture_cache_hits: u64,
    pub texture_cache_misses: u64,
    pub bind_group_cache_size: usize,
    pub bind_group_cache_hits: u64,
    pub bind_group_cache_misses: u64,
    pub total_textures_loaded: u64,
    pub total_frames_rendered: u64,
}

impl std::fmt::Display for GpuMetricsData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let texture_hit_rate = if self.texture_cache_hits + self.texture_cache_misses > 0 {
            (self.texture_cache_hits as f64 / (self.texture_cache_hits + self.texture_cache_misses) as f64) * 100.0
        } else {
            0.0
        };

        let bind_group_hit_rate = if self.bind_group_cache_hits + self.bind_group_cache_misses > 0 {
            (self.bind_group_cache_hits as f64 / (self.bind_group_cache_hits + self.bind_group_cache_misses) as f64) * 100.0
        } else {
            0.0
        };

        write!(f, "GPU Performance Metrics:\n")?;
        write!(f, "  Texture Cache:\n")?;
        write!(f, "    Size: {} textures\n", self.texture_cache_size)?;
        write!(f, "    Hits: {} | Misses: {} | Hit Rate: {:.1}%\n",
               self.texture_cache_hits, self.texture_cache_misses, texture_hit_rate)?;
        write!(f, "  Bind Group Cache:\n")?;
        write!(f, "    Size: {} bind groups\n", self.bind_group_cache_size)?;
        write!(f, "    Hits: {} | Misses: {} | Hit Rate: {:.1}%\n",
               self.bind_group_cache_hits, self.bind_group_cache_misses, bind_group_hit_rate)?;
        write!(f, "  Total Textures Loaded: {}\n", self.total_textures_loaded)?;
        write!(f, "  Total Frames Rendered: {}", self.total_frames_rendered)
    }
}

/// Helper function. Writes a string to the stream. It is required to manually shutdown the stream
/// if desired, this function only handles the writing.
fn write_to_stream(stream: &mut UnixStream, mut s: String) -> color_eyre::Result<()> {
    if !s.ends_with('\n') {
        s.push('\n');
    }
    stream
        .write_all(s.as_bytes())
        .wrap_err("failed to write to socket stream")?;
    stream.flush()?;
    tracing::debug!("wrote to socket stream: {}", s.trim());
    Ok(())
}
