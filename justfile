run:
    RUST_LOG=DEBUG cargo r --release

run-nix: 
    RUST_LOG=DEBUG nix run .

build-nix:
    nix build

clean:
    rm -rf target
    unlink result

sloc:
    wc -l src/*.rs

ping:
    cargo r --bin wayperctl -- ping

ping-nix: build-nix
    ./result/bin/wayperctl ping
