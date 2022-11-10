run:
    RUST_LOG=DEBUG cargo r --release


sloc:
    wc -l src/*.rs

ping:
    cargo r --bin wayperctl -- ping
