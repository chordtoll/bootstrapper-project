set -ex
cd bootstrapper
cargo build --release
cd ..
bootstrapper/target/release/server
