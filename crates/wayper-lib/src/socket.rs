//! Code for the socket daemon and client communication

use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    thread::JoinHandle,
};

use clap::Subcommand;
use color_eyre::eyre::WrapErr;
use serde::{Deserialize, Serialize};
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use thiserror::Error;

/// Helper struct for socket management
pub struct WayperSocket {
    /// The path to the socket
    pub socket_path: PathBuf,
    pub tx: Sender<UnixStream>,
    spawned: bool,
    sender_thread_handle: Option<JoinHandle<Result<(), SocketError>>>,
}

impl WayperSocket {
    /// Creates a new instance of the socket.
    pub fn new(socket_path: PathBuf, tx: Sender<UnixStream>) -> Self {
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
                        Ok((unix_stream, remote_addr)) => {
                            tracing::info!("received socket connection from {remote_addr:?}",);
                            socket_tx.send(unix_stream).unwrap();
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
#[derive(Subcommand, Deserialize, Serialize, strum::Display)]
pub enum SocketCommands {
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
    /// Change profile
    ChangeProfile {
        #[arg(default_value = "default")]
        profile_name: String,
    },
}

impl SocketCommands {
    pub fn from_socket(stream: &mut UnixStream) -> color_eyre::Result<Self> {
        let mut msg = String::new();
        stream
            .read_to_string(&mut msg)
            .context("failed to read the stream")?;

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

#[derive(Error, Debug, Serialize, Deserialize, Clone)]
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
#[derive(Serialize, Deserialize, Debug)]
pub enum SocketOutput {
    /// A string message
    Message(String),
    /// The current wallpaper
    CurrentWallpaper(OutputWallpaper),
    /// List of wallpapers currently loaded
    Wallpapers(Vec<OutputWallpaper>),
    SingleError(SocketError),
    MultipleErrors(Vec<SocketError>),
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
    pub fn from_socket(stream: &mut UnixStream) -> color_eyre::Result<Self> {
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        tracing::debug!("socket replied with response: {response}");
        Self::from_json(&response)
    }
}

/// Struct for wallpaper info from the daemon
#[derive(Deserialize, Serialize, Debug)]
pub struct OutputWallpaper {
    /// The name of the output
    pub output_name: String,
    /// The path to the wallpaper
    pub wallpaper: String,
}

/// Helper function. Writes a string to the stream. It is required to manually shutdown the stream
/// if desired, this function only handles the writing.
fn write_to_stream(stream: &mut UnixStream, mut s: String) -> color_eyre::Result<()> {
    if !s.ends_with('\n') {
        s.push('\n');
    }
    stream
        .write(s.as_bytes())
        .wrap_err("failed to write to socket stream")?;
    tracing::debug!("wrote to socket stream: {}", s.trim());
    Ok(())
}
