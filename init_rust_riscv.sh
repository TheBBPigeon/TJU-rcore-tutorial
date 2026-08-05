# must run after restart terminal

source ~/.cargo/env
rustup target add riscv64gc-unknown-none-elf
rustup component add llvm-tools-preview
rustup component add rust-src
cargo install cargo-binutils --vers =0.3.3