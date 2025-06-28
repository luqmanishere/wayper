default:
    just --list

# build & run wayper with cargo
run-release:
    cargo r --release -- -c samples/test_config.toml

run-ldebug:
    cargo r -- -l debug -c samples/test_config.toml

# build the project with nix
build-nix:
    nix build

# run the wayper daemon built with nix
run-nix: build-nix
    ./result/bin/wayper -c samples/test_config.toml

# run the wayper daemon built with nix, loglevel=debug
run-nix-ldebug: build-nix
    ./result/bin/wayper -l debug -c samples/test_config.toml

# run the wayper daemon built with nix, loglevel=trace
run-nix-ltrace: build-nix
    ./result/bin/wayper -l trace -c samples/test_config.toml

# clean build artifacts
clean:
    rm -rf target
    unlink result

# count the lines of code
sloc:
    wc -l src/*.rs

# run wayperctl's ping command with cargo
ping:
    cargo r --bin wayperctl -- ping

# run wayperctl's ping command with nix
ping-nix: build-nix
    ./result/bin/wayperctl ping

# create new headless output with sway
sway-create:
    direnv exec . swaymsg create_output

# list sway outputs
sway-list-outputs:
    direnv exec . swaymsg -t get_outputs

# record memory and cpu usage - 1 min
psrecord-wayper-60:
    direnv exec . psrecord $(pgrep wayper) --duration 60 --interval 1 --plot plot1.png

htop:
    htop -p $(pgrep wayper)
