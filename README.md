# wayper

A wallpaper setter for wayland. A huge WIP.

It is usable, but there may be many bugs.
## Why
Just a curious foray into the wayland protocol.

## Features
- Sets wallpapers from images
- Rotate through images in a directory with set duration
- Config hotreloads
- Multi-output (displays) support

## Configuration

Config is read from `~/.config/wayper/config.toml`

### Example

``` toml
# table with output's name
[eDP-1]
name = "eDP-1" # optional
path = "path/to/wallpaper/folder/orfile"
duration = 10 # duration between rerenders, or wallpaper switching
```

## Caveats

- Running this as a daemon is a good idea
- So many unwraps.

## Building

1. Get the latest Rust toolchain from your package distribution or [rustup](https://rustup.rs/)
2. Clone the repo.
3. Run `cargo run --release` to run it, or `cargo install --path .` to install via cargo.

## TODO

- Socket for control
- Allow multiple configurations for same output with conditions?
- Execute command/script on wallpaper change

## Inspirations
- [wpaperd](https://github.com/danyspin97/wpaperd)
- [swww](https://github.com/Horus645/swww)

