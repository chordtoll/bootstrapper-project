set -ex
cd bootstrapper
cargo build --release
cd ..
sudo bootstrapper/target/release/worker
