# Amareleo-Chain - disposable, developer-friendly, Aleo chain instances

[![Crates.io](https://img.shields.io/crates/v/amareleo-chain.svg?color=neon)](https://crates.io/crates/amareleo-chain)
[![Authors](https://img.shields.io/badge/authors-Amareleo-orange.svg)](https://amareleo.com)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](./LICENSE.md)

![Amareleo](docs/amareleo.svg)


* [1. Overview](#1-overview)
* [2. Build Guide](#2-build-guide)
    * [2.1 Requirements](#21-Requirements)
    * [2.2 Installation](#22-installation)
* [3. Run](#3-run)
    * [3.1 Amareleo-Chain CLI](#31-amareleo-chain-cli)

<BR />

## 1. Overview

A lite [Aleo](https://aleo.org/) development node. Starting from the [SnarkOS](https://github.com/ProvableHQ/snarkOS) codebase, `amareleo-chain` delivers a minimal validator node for testing the deployment and execution of aleo programs.

Key benefits:
1.	__Ease of Use__ – Starting a development testnet simply involves running `amareleo-chain start`. No extra scripts, no multiple processes, all default parameters set for quick-fire aleo program testing.
 
1.	__Lite__ – Just one process, with minimal memory consumption and processing resources.

1.	__Fast Startup/Shutdown__ – Drastically reduced node startup and shutdown times. 

1.	__Fresh Chain-State__ – Start testing programs with a fresh chain state (default) or retain the chain-state across tests.

1.	__Compatibility__ – Compatible with other Aleo tools including `snarkos` and `leo`.

<BR />

## 2. Build Guide

### 2.1 Requirements

`amareleo-chain` was developed and tested with minimal requriements:

* Ubuntu 22.04 (LTS)
* 11th Gen Intel(R) Core(TM) i7-1165G7 @ 2.80GHz
* 16GB RAM
* 512 GB SSD


### 2.2 Installation

If you already installed `snarkos` on this machine, all dependencies should be satisfied and you can clone and install `amareleo-chain` as follows:

```BASH
git clone https://github.com/kaxxa123/amareleo-chain.git
cd amareleo-chain
cargo install --locked --path .
```

Otherwise, ensure your machine has `Rust v1.81+` installed. Instructions to [install Rust can be found here.](https://www.rust-lang.org/tools/install). Next clone and install `amareleo-chain` as follows:

```BASH
git clone https://github.com/kaxxa123/amareleo-chain.git
cd amareleo-chain
./build_ubuntu.sh
```

## 3. Run

To start a fresh chain from genesis run:
`amareleo-chain start`

This will expose a REST server on [localhost:3030](http://localhost:3030/), supporting the same endpoints as `snarkos`.


### 3.1 Amareleo-Chain CLI

`amareleo-chain` supports the commands:

* `start` - Starts the amareleo-chain node
* `clean` - Cleans the amareleo-chain node storage
* `update` - Update amareleo-chain

For `snarkos` users, running `amareleo-chain start` will look very much like running `snarkos start` with the `--dev 0 --validator` parameters. Some differences include:

* `amareleo-chain` drops a lot of the functionality not relevant to aleo program testing. It only supports running a validator, and only runs as a standalone node without peers.

* The `amareleo-chain` default network is `testnet` (`--network 1`) rather than `mainnet` (`--network 0`).

* `amareleo-chain` supports two modes of operations controlled by the `--keep-state` argument. Running `amareleo-chain start`, the node will start from genesis and will not preserve the chain state accross runs. Running `amareleo-chain start --keep-state` causes it to preserve the chain-state.

* `amareleo-chain` uses different disk storage locations depending on its mode of operation:

    | `--keep-state` | Disk storage             |
    |----------------|--------------------------|
    |  set           | `.amareleo-ledger-*`     |
    |  not set       | `.amareleo-tmp-ledger-*` |

    The two modes do not conflict. One can perform multiple runs some setting `--keep-state`, others not. All the runs specifying `--keep-state` will share the same ledger. This state will keep being preserved until `amareleo-chain clean` is run.

