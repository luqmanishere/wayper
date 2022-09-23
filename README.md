# wayper

A wallpaper setter for wayland. A huge WIP.

It is usable, but there may be many bugs.
## Why
Just a curious foray into the wayland protocol.

## Features
- Sets wallpapers from images
- Rotate images in a directory with set duration
- Config hotreloads

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

- Ideally this should run as a daemon, as the process needs to be alive for wayland to render the
surface.
- So many unwraps.

## Inspirations
- [wpaperd](https://github.com/danyspin97/wpaperd)
- [swww](https://github.com/Horus645/swww)

