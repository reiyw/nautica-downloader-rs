lint:
    cargo +nightly clippy -- -D clippy::all -W clippy::nursery
    cargo +nightly fmt -- --check

fix:
    cargo +nightly clippy --fix -- -D clippy::all -W clippy::nursery
    cargo +nightly fmt
