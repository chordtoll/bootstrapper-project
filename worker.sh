set -ex
cd bootstrapper
cargo build
cd ..
sudo bootstrapper/target/debug/worker
