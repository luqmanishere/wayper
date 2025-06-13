use std::os::unix::net::UnixStream;

use clap::Parser;
use color_eyre::eyre::{Result, WrapErr, eyre};
use tracing::{info, level_filters::LevelFilter};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt};
use wayper_lib::socket::{SocketCommands, SocketError, SocketOutput};

fn main() -> Result<()> {
    color_eyre::install()?;

    // Do not drop the guards until the program exists
    let _guards = start_logger();

    let mut cli = Cli::parse();

    let socket_path = if let Some(path) = cli.socket_path.take() {
        path
    } else {
        "/tmp/wayper/.socket.sock".to_string()
    };

    let mut stream = UnixStream::connect(&socket_path)
        .wrap_err_with(|| eyre!("could not connect to wayper socket at {socket_path}"))?;
    match cli.command {
        SocketCommands::Ping => {
            cli.command.write_to_socket(&mut stream)?;
            stream.shutdown(std::net::Shutdown::Write)?;

            let output = SocketOutput::from_socket(&mut stream)?;
            handle_error_from_daemon(&output)?;

            if let SocketOutput::Message(msg) = output {
                println!("{msg}");
                tracing::info!("{msg}");
            } else {
                failed_to_get_response()?;
            }
        }
        SocketCommands::Current { .. } => {
            cli.command.write_to_socket(&mut stream)?;
            stream.shutdown(std::net::Shutdown::Write)?;

            let output = SocketOutput::from_socket(&mut stream)?;
            handle_error_from_daemon(&output)?;

            match output {
                SocketOutput::CurrentWallpaper(output_wallpaper) => println!(
                    "{}: {}",
                    output_wallpaper.output_name, output_wallpaper.wallpaper
                ),
                SocketOutput::Wallpapers(output_wallpapers) => {
                    for output_wallpaper in output_wallpapers {
                        println!(
                            "{}: {}",
                            output_wallpaper.output_name, output_wallpaper.wallpaper
                        );
                    }
                }
                _ => {}
            };
        }
        SocketCommands::Toggle { .. } => {
            cli.command.write_to_socket(&mut stream)?;
            stream.shutdown(std::net::Shutdown::Write)?;

            let output = SocketOutput::from_socket(&mut stream)?;
            handle_error_from_daemon(&output)?;

            // TODO: output
            if let SocketOutput::Message(msg) = output {
                println!("{msg}");
                tracing::info!("{msg}");
            } else {
                failed_to_get_response()?;
            }
        }
        // this is also a template for handling commands
        command => {
            command.write_to_socket(&mut stream)?;
            // you can write multiple things and wait for multiple things as well
            // before shutting down our side of the socket
            stream.shutdown(std::net::Shutdown::Write)?;

            let output = SocketOutput::from_socket(&mut stream)?;
            handle_error_from_daemon(&output)?;

            // put command specific parsing here
        }
    }

    Ok(())
}

/// Handle errors from the daemon
fn handle_error_from_daemon(output: &SocketOutput) -> Result<(), SocketError> {
    if let SocketOutput::SingleError(error) = output {
        tracing::error!("daemon returned an error: {error}");

        return Err(error.clone());
    }
    Ok(())
}

fn failed_to_get_response() -> Result<()> {
    tracing::error!("failed to get response");
    Err(eyre!("failed to get response"))
}

fn start_logger() -> Vec<WorkerGuard> {
    let mut guards = Vec::new();
    let file_appender = tracing_appender::rolling::never("/tmp/wayper", "wayperctl-log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    // tracing_appender::non_blocking::NonBlockingBuilder
    guards.push(guard);

    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::Layer::new()
            .with_writer(non_blocking)
            .with_ansi(false)
            // .with_timer(tracing_subscriber::fmt::time::time())
            .with_filter(
                EnvFilter::builder()
                    .with_env_var("RUST_LOG")
                    .with_default_directive(LevelFilter::DEBUG.into())
                    .from_env_lossy(),
            ),
    );

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
    info!("logger started!");
    guards
}

/// A tool to communicate with the wayper daemon through the socket. You can also
/// do this with any other unix socket tool, like socat.
///
/// For programmatical use, a json output option is provided. There is no guarantee
/// that the interface will remain the same until 1.0 (as if that will ever happen).
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Set the socket path for wayper
    #[arg(short, long)]
    socket_path: Option<String>,

    /// Whether to output in JSON
    #[arg(short, long, default_value_t = false)]
    json: bool,

    #[command(subcommand)]
    command: SocketCommands,
}
