use std::os::unix::net::UnixStream;

use clap::{CommandFactory, Parser};
use color_eyre::eyre::{Result, WrapErr, eyre};
use tracing::{info, level_filters::LevelFilter};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt};
use wayper_lib::socket::{SocketCommand, SocketError, SocketOutput};

fn main() -> Result<()> {
    clap_complete::CompleteEnv::with_factory(|| Cli::command().bin_name("wayperctl")).complete();
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
        Commands::Socket(ref command) => {
            command.write_to_socket(&mut stream)?;
            match command {
                SocketCommand::Ping => {
                    let replies = SocketOutput::from_socket(&mut stream)?;
                    for reply in replies {
                        handle_error_from_daemon(&cli, &reply)?;

                        if let SocketOutput::Message(ref msg) = reply {
                            if cli.json {
                                println!("{}", reply.to_json()?)
                            } else {
                                println!("{msg}");
                            }
                            tracing::info!("{msg}");
                        } else {
                            failed_to_get_response()?;
                        }
                    }
                }
                SocketCommand::Current { .. } => {
                    let replies = SocketOutput::from_socket(&mut stream)?;
                    for reply in replies {
                        handle_error_from_daemon(&cli, &reply)?;

                        match reply {
                            SocketOutput::CurrentWallpaper(ref output_wallpaper) => {
                                if cli.json {
                                    println!("{}", reply.to_json()?);
                                } else {
                                    println!(
                                        "{}: {}",
                                        output_wallpaper.output_name, output_wallpaper.wallpaper
                                    );
                                }
                            }
                            SocketOutput::Wallpapers(ref output_wallpapers) => {
                                for output_wallpaper in output_wallpapers {
                                    if cli.json {
                                        println!("{}", reply.to_json()?);
                                    } else {
                                        println!(
                                            "{}: {}",
                                            output_wallpaper.output_name,
                                            output_wallpaper.wallpaper
                                        );
                                    }
                                }
                            }
                            _ => {}
                        };
                    }
                }
                SocketCommand::Toggle { .. } => {
                    let replies = SocketOutput::from_socket(&mut stream)?;
                    for reply in replies {
                        handle_error_from_daemon(&cli, &reply)?;

                        // TODO: output
                        if let SocketOutput::Message(ref msg) = reply {
                            if cli.json {
                                println!("{}", reply.to_json()?);
                            } else {
                                println!("{msg}");
                            }
                            tracing::info!("{msg}");
                        } else {
                            failed_to_get_response()?;
                        }
                    }
                }
                SocketCommand::ChangeProfile { .. } => {
                    let replies = SocketOutput::from_socket(&mut stream)?;
                    for reply in replies {
                        handle_error_from_daemon(&cli, &reply)?;

                        if let SocketOutput::Message(ref msg) = reply {
                            if cli.json {
                                println!("{}", reply.to_json()?);
                            } else {
                                println!("{msg}");
                            }
                        } else {
                            failed_to_get_response()?;
                        }
                    }
                }
                SocketCommand::Profiles => {
                    let replies = SocketOutput::from_socket(&mut stream)?;

                    for reply in replies {
                        handle_error_from_daemon(&cli, &reply)?;
                        if let SocketOutput::Profiles(ref profiles) = reply {
                            if cli.json {
                                println!("{}", reply.to_json().expect("convert to json"));
                            } else {
                                println!("Available profiles: {}", profiles.join(", "));
                            }
                        } else {
                            failed_to_get_response()?;
                        }
                    }
                }
                SocketCommand::GpuMetrics => {
                    let replies = SocketOutput::from_socket(&mut stream)?;

                    for reply in replies {
                        handle_error_from_daemon(&cli, &reply)?;
                        if let SocketOutput::GpuMetrics(ref metrics) = reply {
                            if cli.json {
                                println!("{}", reply.to_json().expect("convert to json"));
                            } else {
                                println!("{}", metrics);
                            }
                        } else {
                            failed_to_get_response()?;
                        }
                    }
                }
                // this is also a template for handling commands
                ref command => {
                    let replies = SocketOutput::from_socket(&mut stream)?;
                    for reply in replies {
                        handle_error_from_daemon(&cli, &reply)?;
                        if let SocketOutput::Message(ref msg) = reply {
                            if cli.json {
                                println!("{}", reply.to_json()?);
                            } else {
                                println!("Output from command {command}:\n{msg}");
                            }
                        }

                        // put command specific parsing here
                    }
                }
            }
        }
        Commands::Completions { shell } => {
            println!("{shell}");
            clap_complete::generate(
                clap_complete::shells::Fish,
                &mut Cli::command(),
                "wayperctl",
                &mut std::io::stdout(),
            );
        }
    }

    Ok(())
}

/// Handle errors from the daemon
fn handle_error_from_daemon(cli: &Cli, output: &SocketOutput) -> Result<(), SocketError> {
    if let SocketOutput::SingleError(error) = output {
        if cli.json {
            println!("{}", output.to_json().expect("json conversion"));
        }
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
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    #[command(flatten)]
    Socket(SocketCommand),
    /// Generate shell completions for a supported shell.
    Completions { shell: clap_complete::Shell },
}
