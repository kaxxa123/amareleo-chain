# Installing Amareleo-Chain

1. [Overview](./00_introduction.md)
1. Installation
1. [Running](./02_running.md)
1. [Amareleo-Chain vs SnarkOS](./03_differences.md)

---

Amareleo-Chain may be installed from source or from [crates.io](https://crates.io/) using cargo. 
On a fresh machine, installing from source is recommanded as this will also help getting all dependencies installed.


## Requirements

`amareleo-chain` was tested on machines with low end specs.

* Ubuntu 22.04 (LTS)
* 11th Gen Intel(R) Core(TM) i7-1165G7 @ 2.80GHz
* 16GB RAM
* 512 GB SSD

## Install from source

Ensure your machine has `Rust v1.81+` installed. Instructions to [install Rust can be found here](https://www.rust-lang.org/tools/install). Next clone and install `amareleo-chain` as follows:

```BASH
git clone https://github.com/kaxxa123/amareleo-chain.git
cd amareleo-chain
./build_ubuntu.sh

# Confirm installation
amareleo-chain help
```


