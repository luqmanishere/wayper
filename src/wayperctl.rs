use std::{
    collections::HashMap,
    io::{Read, Write},
    os::unix::net::UnixStream,
};

use clap::{ArgMatches, Command};
use eyre::{eyre, Result, WrapErr};
use tracing::{info, Level};

fn main() -> Result<()> {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;
    let matches = setup_cli();
    match matches.subcommand_name() {
        Some("ping") => {
            let socket_path = "/tmp/wayper/.socket.sock";
            let mut unix_stream = UnixStream::connect(socket_path)
                .wrap_err_with(|| eyre!("could not connect to wayper socket at {socket_path}"))?;

            unix_stream
                .write(b"ping")
                .wrap_err("failed to write to wayper socket")?;
            unix_stream.shutdown(std::net::Shutdown::Write)?;

            let mut response = String::new();
            unix_stream.read_to_string(&mut response)?;
            info!("got response: {response}");
        }
        Some("current") => {
            let socket_path = "/tmp/wayper/.socket.sock";
            let mut unix_stream = UnixStream::connect(socket_path)
                .wrap_err_with(|| eyre!("could not connect to wayper socket at {socket_path}"))?;

            unix_stream
                .write(b"current")
                .wrap_err("failed to write to wayper socket")?;
            unix_stream.shutdown(std::net::Shutdown::Write)?;

            let mut response = String::new();
            unix_stream.read_to_string(&mut response)?;
            let vect: Vec<_> = response.lines().map(|s| s.to_string()).collect();
            let map: HashMap<_, _> = vect
                .chunks_exact(2)
                .map(|c| (c[0].clone(), c[1].clone()))
                .collect();
            for (k, v) in map {
                println!("{}: {}", k, v);
            }
        }
        Some(_) => {}
        None => {}
    }
    Ok(())
}

fn setup_cli() -> ArgMatches {
    Command::new("wayperctl")
        .subcommand(Command::new("ping"))
        .subcommand(Command::new("current"))
        .get_matches()
}
