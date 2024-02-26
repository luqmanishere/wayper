# wayper

A wallpaper setter for wayland. A huge WIP.

It is usable, but there may be many bugs.

## Why

Just a curious foray into the wayland protocol.

Update 2024: I still don't understand much, especially when jumping back into this project after a year of absence.

## Features

- Sets wallpapers from images
- Rotate through images in a directory with set duration
- Config hotreloads (currently in reimplementation)
- Multi-output (displays) multi-image support

## Configuration

Config is read from `~/.config/wayper/config.toml`

### Example

```toml
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

### Generic Linux Distribution

1. Get the latest stable Rust toolchain from your package distribution or [rustup](https://rustup.rs/)
2. Clone the repo.
3. Run `cargo run --release` to run it, or `cargo install --path .` to install via cargo.

### Nix Flakes

1. Set this in your flake inputs:

```nix
inputs = {
    # append a /<commit hash/branch/tag> to lock that specific hash
    wayper.url = "github:luqmanishere/wayper";
}
```

2. Use the provided Home Manager module (exported under homeManagerModules) to configure it as a service.
3. NixOS module coming soonish.

## What's Next?

- Video/GIFs/Animated wallpapers
- Those transitions look cool af too
- Socket for control
- Maybe allow multiple configurations for same output with conditions
- Execute command/script on wallpaper change

- NixOS module

## Inspirations

- [wpaperd](https://github.com/danyspin97/wpaperd)
- [swww](https://github.com/Horus645/swww)
