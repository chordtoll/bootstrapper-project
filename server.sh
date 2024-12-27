set -ex
cd bootstrapper
cargo build
cd ..
bootstrapper/target/debug/server
