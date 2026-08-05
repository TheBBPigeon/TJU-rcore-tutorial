# update software source
sudo apt update
sudo apt upgrade -y

# install basic tools
sudo apt install -y git build-essential curl wget libssl-dev pkg-config
# install rust tool chains
curl https://sh.rustup.rs -sSf | sh