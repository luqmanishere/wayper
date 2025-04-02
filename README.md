# wayper

[![](https://tokei.rs/b1/github/luqmanishere/wayper)](https://github.com/luqmanishere/wayper)

A wallpaper daemon for wayland. Huge WIP. Many, many bugs.

## Why

Just a curious foray into the wayland protocol.

Update 2024: I still don't understand much, especially when jumping back into this project after a year of absence.
Update 2025: I found I broke a bunch of shit idk when, now i'm fixing all that functionality again...

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

- Many unfinished features.

## Building

### Generic Linux Distribution

1. Get the latest stable Rust toolchain from your package distribution or [rustup](https://rustup.rs/)
2. Clone the repo.
3. Run `cargo run --release` to run it, or `cargo install --path .` to install via cargo.
4. Or you could `cargo install --git`.

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

## Testing

### Outputs: Connection, Disconnection, etc
Testing can be done in any Wayland environment (untested on KDE & GNOME). To test code regarding outputs,
it can be done with `sway`'s headless outputs. You can run sway within an already running Wayland env,
launch foot with the default keybindings, spawn outputs and run your tests there. The development
Nix flake and `justfile` facilitates this method of testing

## What's Next?

- Video/GIFs/Animated wallpapers
- Those transitions look cool af too
- Socket control
- Profiles, conditions
- Execute command/script on wallpaper change
- NixOS module

## Inspirations

- [wpaperd](https://github.com/danyspin97/wpaperd)
- [swww](https://github.com/Horus645/swww)
