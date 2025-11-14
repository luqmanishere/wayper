# Wayper Home Manager Module

This directory contains the Home Manager module for Wayper, a Wayland wallpaper daemon.

## Usage

### Basic Setup

Add Wayper to your Home Manager configuration:

```nix
{
  services.wayper = {
    enable = true;
    config = {
      default-profile = "default";
      transitions_enabled = true;

      # Global transition configuration
      transition = {
        type = "crossfade";
        duration_ms = 2000;
        fps = 60;
      };

      # Monitor configurations
      monitorConfigs = [{
        name = "eDP-1";
        profile = "default";
        duration = 30;  # seconds between wallpaper changes
        path = "/home/user/wallpapers";
      }];
    };
  };
}
```

### Configuration Options

#### Global Options

- `services.wayper.enable` - Enable the Wayper service
- `services.wayper.package` - The Wayper package to use (auto-detected from flake)
- `services.wayper.enableFuzzelIntegration` - Enable fuzzel launcher integration
- `services.wayper.enableFishCompletions` - Install fish shell completions

#### Config Options

- `config.default-profile` (string) - The default profile name
- `config.transitions_enabled` (bool) - Globally enable/disable transitions

#### Global Transition Options

- `config.transition.type` (string or list of strings) - Transition type:
  - `"crossfade"` - Smooth crossfade between wallpapers
  - `"sweep"` - Directional sweep transition
  - `["crossfade" "sweep"]` - Random selection from list

- `config.transition.duration_ms` (int, optional) - Transition duration in milliseconds
- `config.transition.fps` (int, optional) - Frames per second for transitions

#### Sweep-Specific Options

- `config.transition.sweep.direction` (enum, optional) - Sweep direction:
  - `"left-to-right"`
  - `"right-to-left"`
  - `"top-to-bottom"`
  - `"bottom-to-top"`
  - `"top-left-to-bottom-right"`
  - `"top-right-to-bottom-left"`
  - `"bottom-left-to-top-right"`
  - `"bottom-right-to-top-left"`

- `config.transition.sweep.edge_width` (float, optional) - Sweep edge width (0.0-1.0), default: 0.05

#### Per-Monitor Options

Each monitor in `config.monitorConfigs` can have:

- `name` (string) - Monitor name (e.g., "eDP-1", "HDMI-A-1")
- `profile` (string) - Profile name for this monitor
- `duration` (int) - Seconds between wallpaper changes
- `path` (string) - Path to wallpaper file(s)
- `run_command` (string, optional) - Command to run when wallpaper changes
- `transitions_enabled` (bool, optional) - Override global transitions setting
- `transition` (submodule, optional) - Per-monitor transition config (same options as global)

### Advanced Examples

#### Multiple Monitors with Different Transitions

```nix
{
  services.wayper = {
    enable = true;
    config = {
      default-profile = "work";
      transitions_enabled = true;

      # Global fallback transition
      transition = {
        type = "crossfade";
        duration_ms = 2000;
        fps = 60;
      };

      monitorConfigs = [
        {
          name = "eDP-1";
          profile = "laptop";
          duration = 300;
          path = "/home/user/wallpapers/laptop";
          transition = {
            type = "sweep";
            duration_ms = 1500;
            fps = 60;
            sweep = {
              direction = "left-to-right";
              edge_width = 0.05;
            };
          };
        }
        {
          name = "HDMI-A-1";
          profile = "external";
          duration = 600;
          path = "/home/user/wallpapers/4k";
          transition = {
            type = "crossfade";
            duration_ms = 3000;
            fps = 30;
          };
        }
      ];
    };
  };
}
```

#### Random Transition Selection

```nix
{
  services.wayper.config = {
    transition = {
      type = ["crossfade" "sweep"];  # Randomly picks one
      duration_ms = 1500;
      fps = 60;
      sweep = {
        direction = "top-to-bottom";
      };
    };
  };
}
```

#### Disable Transitions for Specific Monitor

```nix
{
  services.wayper.config = {
    transitions_enabled = true;  # Global setting

    monitorConfigs = [
      {
        name = "eDP-1";
        profile = "default";
        duration = 30;
        path = "/home/user/wallpapers";
        transitions_enabled = false;  # Override for this monitor
      }
    ];
  };
}
```

## Testing

### Standalone Build Test

Generate TOML configs from test scenarios:

```bash
cd nix
nix-build test.nix
```

This creates a `result/` directory with generated TOML files:
- `wayper-basic.toml` - Basic crossfade configuration
- `wayper-sweep.toml` - Sweep transition with direction
- `wayper-multi-monitor.toml` - Multiple monitors
- `wayper-mixed.toml` - Mixed transition types
- `wayper-random.toml` - Random transition selection
- `wayper-minimal.toml` - Minimal config without transitions
- `wayper-comprehensive.toml` - All features enabled

Inspect generated configs:

```bash
cat result/wayper-basic.toml
cat result/wayper-sweep.toml
```

### Assertion-Based Tests

Run automated tests to verify correctness:

```bash
cd nix
nix eval -f test-assertions.nix
```

Expected output:
```
{ message = "All 10 tests passed!"; success = true; results = [...]; }
```

If tests fail, you'll see detailed error messages indicating which assertions failed.

### What the Tests Verify

The test suite validates:
1. Proper `[transition]` block generation (not flat fields)
2. Sweep configuration with nested `[transition.sweep]` blocks
3. Per-monitor transition blocks: `[profile.output.transition]`
4. Random transition type array syntax
5. Absence of transition blocks when not configured
6. Per-monitor `transitions_enabled` flag
7. All 8 sweep directions work correctly
8. Multiple monitor config blocks
9. `run_command` field inclusion
10. Field naming uses underscores (not hyphens)

## Troubleshooting

### Generated Config Location

The module generates config at:
```
~/.config/wayper/config.toml
```

### Verify Generated Config

```bash
cat ~/.config/wayper/config.toml
```

### Service Status

```bash
systemctl --user status wayper
```

### View Logs

```bash
journalctl --user -u wayper -f
```

### Common Issues

**Problem**: Transitions not working
**Solution**: Check that `transitions_enabled = true` and transition config is properly set

**Problem**: Wrong TOML syntax
**Solution**: Run the test suite to verify module generates correct TOML

**Problem**: Service fails to start
**Solution**: Check that wallpaper paths exist and are accessible

## Development

### Module Structure

- `hm-module.nix` - Main Home Manager module
- `test.nix` - Standalone build tests
- `test-assertions.nix` - Automated assertion tests
- `README.md` - This file

### Making Changes

1. Edit `hm-module.nix`
2. Run tests: `nix-build test.nix && nix eval -f test-assertions.nix`
3. Verify generated TOML matches expected format
4. Test in real Home Manager configuration

### Adding New Options

1. Add option to the appropriate submodule in `hm-module.nix`
2. Update TOML generation logic
3. Add test case to `test.nix`
4. Add assertion to `test-assertions.nix`
5. Update this README with documentation
