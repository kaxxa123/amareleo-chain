#!/bin/bash
echo "======================================================"
echo " Attention - Building amareleo-chain from source code."
echo " This will request root permissions with sudo."
echo "======================================================"

# Install Ubuntu dependencies

sudo apt-get update
sudo apt-get install -y \
	build-essential \
	curl \
	clang \
	gcc \
	libssl-dev \
	llvm \
	make \
	pkg-config \
	tmux \
	xz-utils \
	ufw


# Install Rust

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source $HOME/.cargo/env

# Install amareleo-chain
# cargo clean
cargo install --locked --path .

echo "=================================================="
echo " Attention - Please ensure port 3030 is enabled "
echo "             on your local network."
echo ""
echo " Cloud Providers - Enable port 3030 in your"
echo "                   network firewall"
echo ""
echo " Home Users - Enable port forwarding or NAT rule"
echo "              for 3030 on your router."
echo "=================================================="
