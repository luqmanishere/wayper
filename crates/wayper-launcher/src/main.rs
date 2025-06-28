//! This is a WIP

use std::{
    io::{BufRead, Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::Stdio,
    str::FromStr,
};

use clap::Parser;
use color_eyre::eyre::{Context, ContextCompat, eyre};
use serde::{Deserialize, Serialize};
use strum::VariantNames;
use wayper_lib::socket::{SocketCommand, SocketOutput};

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let mut cli = Cli::parse();

    let socket_path = if let Some(path) = cli.socket_path.take() {
        path
    } else {
        "/tmp/wayper/.socket.sock".to_string()
    };

    let mut stream = UnixStream::connect(&socket_path)
        .wrap_err_with(|| eyre!("could not connect to wayper socket at {socket_path}"))?;

    SocketCommand::Profiles.write_to_socket(&mut stream)?;

    let SocketOutput::Profiles(profiles) = SocketOutput::from_socket(&mut stream)?
        .into_iter()
        .find(|e| matches!(e, SocketOutput::Profiles(_)))
        .wrap_err("unable to get correct output")?
    else {
        return Err(eyre!("socket error"));
    };

    Prompt::new(profiles, cli.launcher.to_launcher(None), stream).run()?;
    Ok(())
}

pub struct Prompt {
    launcher: Launcher,
    state: PromptState,
    profiles: Vec<String>,
    socket: UnixStream,
    last_output: Option<String>,
}

impl Prompt {
    pub fn new(profiles: Vec<String>, launcher: Launcher, socket_path: UnixStream) -> Self {
        Self {
            profiles,
            launcher,
            state: PromptState::default(),
            socket: socket_path,
            last_output: None,
        }
    }

    fn build(&mut self) -> Option<String> {
        let mut prompt: Vec<String> = vec![];
        match self.state {
            PromptState::Main => {
                self.profiles
                    .iter()
                    .for_each(|e| prompt.push(e.to_string()));
                prompt.push("Commands".to_string());
                prompt.push("Settings".to_string());

                prompt.push("Exit".to_string());
                Some(prompt.join("\n"))
            }
            PromptState::Commands => {
                SocketCommand::VARIANTS
                    .iter()
                    .filter(|e| **e != "change-profile")
                    .for_each(|e| prompt.push(e.to_string()));

                prompt.push("Back".to_string());
                Some(prompt.join("\n"))
            }
            PromptState::Settings => {
                prompt.push("Work in progress".to_string());

                prompt.push("Back".to_string());
                Some(prompt.join("\n"))
            }
        }
    }

    pub fn run(&mut self) -> color_eyre::Result<()> {
        // TODO: macros may make this easier
        while let Some(prompt) = self.build() {
            let (output_reader, output_writer) = std::io::pipe()?;
            let mut output = String::new();

            // spawn the process
            let mut process = std::process::Command::new(self.launcher.to_string());
            match self.launcher {
                Launcher::Fuzzel => {
                    process.arg("-d");
                }
                Launcher::Custom(_) => {}
            }

            let mut process = process
                .stdin({
                    let (reader, mut writer) = std::io::pipe()?;
                    writer.write_all(prompt.as_bytes())?;
                    Stdio::from(reader)
                })
                .stdout(output_writer)
                .stderr(Stdio::inherit())
                .spawn()?;

            // read_ blocks when empty, so if the process fails we just bail
            let code = process.wait()?;
            if !code.success() {
                break;
            }

            let mut reader = std::io::BufReader::new(output_reader);
            reader.read_line(&mut output)?;
            let output = output.trim().to_lowercase();

            // process the input
            match self.state {
                PromptState::Main => {
                    match output.as_str() {
                        "settings" => {
                            self.state = PromptState::Settings;
                        }
                        "commands" => {
                            self.state = PromptState::Commands;
                        }
                        "exit" => {
                            break;
                        }
                        // assume any other options are profiles
                        profile => {
                            if profile.is_empty() {
                                break;
                            }
                            let command = SocketCommand::ChangeProfile {
                                profile_name: Some(profile.to_string()),
                            };
                            let socket = &mut self.socket;

                            command.write_to_socket(socket)?;

                            let output = SocketOutput::from_socket(socket)?;
                            for output in output {
                                if let SocketOutput::SingleError(error) = output {
                                    // tracing::error!("daemon returned an error: {error}");

                                    return Err(error.clone().into());
                                } else {
                                    println!("{output}");
                                }
                            }

                            break;
                        }
                    }
                }
                PromptState::Commands => {
                    match output.as_str() {
                        "back" => self.state = PromptState::Main,
                        command => {
                            let command = SocketCommand::from_str(command)?;
                            let socket = &mut self.socket;

                            command.write_to_socket(socket)?;

                            let replies = SocketOutput::from_socket(socket)?;
                            for reply in replies {
                                if let SocketOutput::SingleError(error) = reply {
                                    // tracing::error!("daemon returned an error: {error}");

                                    return Err(error.clone().into());
                                }
                                println!("{reply}");
                            }

                            // let output = SocketOutput::from_socket(&mut socket)?;
                            break;
                        }
                    };
                }
                PromptState::Settings => {
                    match output.as_str() {
                        "back" | "work in progress" => self.state = PromptState::Main,
                        _setting => {
                            todo!()
                        }
                    };
                }
            }
            self.last_output = Some(output);
        }
        Ok(())
    }
}

#[derive(Default)]
pub enum PromptState {
    #[default]
    Main,
    Commands,
    Settings,
}

/// Launcher representation for the prompt builder. Seperate from cli args due to enum variant limitations
#[derive(strum::Display, Clone)]
#[strum(serialize_all = "lowercase")]
pub enum Launcher {
    Fuzzel,
    #[strum(to_string = "{0}")]
    Custom(String),
}

#[derive(Parser)]
#[command(version, about)]
pub struct Cli {
    /// Set the socket path for wayper
    #[arg(short, long)]
    socket_path: Option<String>,

    /// The launcher to launch. The custom option should be accompanied with --launcher-command
    #[arg(short, long, required = true)]
    launcher: CliLauncher,

    /// Specify the launcher command.
    /// Overrides the default launcher type command. Useful with the custom launcher option
    #[arg(long)]
    launcher_command: Option<String>,
}

#[derive(Clone, clap::ValueEnum)]
enum CliLauncher {
    Fuzzel,
    Custom,
}

impl CliLauncher {
    pub fn to_launcher(&self, custom: Option<String>) -> Launcher {
        match self {
            CliLauncher::Fuzzel => Launcher::Fuzzel,
            CliLauncher::Custom => Launcher::Custom(custom.expect("custom launcher must have arg")),
        }
    }
}
