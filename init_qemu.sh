# install qemu 7.0.0 which adapt to r-core
sudo apt install -y autoconf automake autotools-dev curl libmpc-dev libmpfr-dev libgmp-dev \
  gawk build-essential bison flex texinfo gperf libtool patchutils bc \
  zlib1g-dev libexpat-dev pkg-config libglib2.0-dev libpixman-1-dev git tmux python3 python3-pip ninja-build

# download qemu 7.0.0 source code
wget https://download.qemu.org/qemu-7.0.0.tar.xz
tar xvJf qemu-7.0.0.tar.xz
cd qemu-7.0.0
./configure --target-list=riscv64-softmmu,riscv64-linux-user
make -j$(nproc)

rm -rf ~/qemu-7.0.0
rm ~/qemu-7.0.0.tar.xz

# restart terminal