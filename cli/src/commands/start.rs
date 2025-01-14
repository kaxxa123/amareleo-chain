// Copyright 2024 Aleo Network Foundation
// This file is part of the snarkOS library.

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at:

// http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::{bail, ensure, Result};
use clap::Parser;
use colored::Colorize;
use core::str::FromStr;
use indexmap::IndexMap;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaChaRng;

use snarkos_lite_account::Account;
use snarkos_lite_node::bft::helpers::amareleo_storage_mode;
use snarkos_lite_node::Node;
use snarkos_lite_resources::{
    BLOCK0_CANARY, BLOCK0_CANARY_ID, BLOCK0_MAINNET, BLOCK0_MAINNET_ID, BLOCK0_TESTNET,
    BLOCK0_TESTNET_ID,
};

use snarkvm::{
    console::{
        account::{Address, PrivateKey},
        algorithms::Hash,
        network::{CanaryV0, MainnetV0, Network, TestnetV0},
    },
    ledger::{
        block::Block,
        committee::{Committee, MIN_VALIDATOR_STAKE},
        store::{helpers::memory::ConsensusMemory, ConsensusStore},
    },
    prelude::{FromBytes, ToBits, ToBytes},
    synthesizer::VM,
    utilities::to_bytes_le,
};
use std::result::Result::Ok;

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
};
use tokio::runtime::{self, Runtime};

/// The development mode RNG seed.
const DEVELOPMENT_MODE_RNG_SEED: u64 = 1234567890u64;
/// The development mode number of genesis committee members.
const DEVELOPMENT_MODE_NUM_GENESIS_COMMITTEE_MEMBERS: u16 = 4;

/// Starts the amareleo-chain node.
#[derive(Clone, Debug, Parser)]
pub struct Start {
    /// Specify the network ID of this node
    #[clap(default_value = "1", long = "network")]
    pub network: u16,

    /// Specify the IP address and port for the node server
    #[clap(long = "node")]
    pub node: Option<SocketAddr>,

    /// Specify the IP address and port for the REST server
    #[clap(long = "rest")]
    pub rest: Option<SocketAddr>,
    /// Specify the requests per second (RPS) rate limit per IP for the REST server
    #[clap(default_value = "10", long = "rest-rps")]
    pub rest_rps: u32,

    /// Specify the verbosity of the node [options: 0, 1, 2, 3, 4]
    #[clap(default_value = "1", long = "verbosity")]
    pub verbosity: u8,
    /// Specify the path to the file where logs will be stored
    #[clap(default_value_os_t = std::env::temp_dir().join("amareleo-chain.log"), long = "logfile")]
    pub logfile: PathBuf,

    /// Enables the metrics exporter
    #[clap(default_value = "false", long = "metrics")]
    pub metrics: bool,
    /// Specify the IP address and port for the metrics exporter
    #[clap(long = "metrics-ip")]
    pub metrics_ip: Option<SocketAddr>,

    /// Specify the path to a directory containing the storage database for the ledger
    #[clap(long = "storage")]
    pub storage: Option<PathBuf>,

    /// If developtment mode is enabled, specify whether it should generate traffic to drive the network
    #[clap(default_value = "false", long = "no-dev-txs")]
    pub no_dev_txs: bool,
}

impl Start {
    /// Starts the amareleo-chain node.
    pub fn parse(self) -> Result<String> {
        // Prepare the shutdown flag.
        let shutdown: Arc<AtomicBool> = Default::default();

        // Initialize the logger.
        crate::helpers::initialize_logger(self.verbosity, self.logfile.clone(), shutdown.clone());
        // Initialize the runtime.
        Self::runtime().block_on(async move {
            // Clone the configurations.
            let mut cli = self.clone();
            // Parse the network.
            match cli.network {
                MainnetV0::ID => {
                    // Parse the node from the configurations.
                    cli.parse_node::<MainnetV0>(shutdown.clone())
                        .await
                        .expect("Failed to parse the node");
                }
                TestnetV0::ID => {
                    // Parse the node from the configurations.
                    cli.parse_node::<TestnetV0>(shutdown.clone())
                        .await
                        .expect("Failed to parse the node");
                }
                CanaryV0::ID => {
                    // Parse the node from the configurations.
                    cli.parse_node::<CanaryV0>(shutdown.clone())
                        .await
                        .expect("Failed to parse the node");
                }
                _ => panic!("Invalid network ID specified"),
            };
            // Note: Do not move this. The pending await must be here otherwise
            // other snarkOS commands will not exit.
            std::future::pending::<()>().await;
        });

        Ok(String::new())
    }
}

impl Start {
    /// Compute fixed development node private key.
    fn parse_private_key<N: Network>(&self) -> Result<Account<N>> {
        // Sample the private key of this node.
        Account::try_from({
            // Initialize the (fixed) RNG.
            let mut rng = ChaChaRng::seed_from_u64(DEVELOPMENT_MODE_RNG_SEED);

            let private_key = PrivateKey::<N>::new(&mut rng)?;
            println!(
                "🔑 Your development private key for node 0 is {}.\n",
                private_key.to_string().bold()
            );
            private_key
        })
    }

    /// Updates the configurations if the node is in development mode.
    fn parse_development(&mut self) -> Result<()> {
        // If the REST IP is not already specified set the REST IP to `3030`.
        if self.rest.is_none() {
            self.rest = Some(SocketAddr::from_str(&format!("0.0.0.0:{}", 3030)).unwrap());
        }

        Ok(())
    }

    /// Returns an alternative genesis block if the node is in development mode.
    /// Otherwise, returns the actual genesis block.
    fn parse_genesis<N: Network>(&self) -> Result<Block<N>> {
        // Initialize the (fixed) RNG.
        let mut rng = ChaChaRng::seed_from_u64(DEVELOPMENT_MODE_RNG_SEED);

        // Initialize the development private keys.
        let development_private_keys = (0..DEVELOPMENT_MODE_NUM_GENESIS_COMMITTEE_MEMBERS)
            .map(|_| PrivateKey::<N>::new(&mut rng))
            .collect::<Result<Vec<_>>>()?;
        // Initialize the development addresses.
        let development_addresses = development_private_keys
            .iter()
            .map(Address::<N>::try_from)
            .collect::<Result<Vec<_>>>()?;

        let (committee, bonded_balances) = {
            // Calculate the committee stake per member.
            let stake_per_member = N::STARTING_SUPPLY
                .saturating_div(2)
                .saturating_div(DEVELOPMENT_MODE_NUM_GENESIS_COMMITTEE_MEMBERS as u64);
            ensure!(
                stake_per_member >= MIN_VALIDATOR_STAKE,
                "Committee stake per member is too low"
            );

            // Construct the committee members and distribute stakes evenly among committee members.
            let members = development_addresses
                .iter()
                .map(|address| (*address, (stake_per_member, true, rng.gen_range(0..100))))
                .collect::<IndexMap<_, _>>();

            // Construct the bonded balances.
            // Note: The withdrawal address is set to the staker address.
            let bonded_balances = members
                .iter()
                .map(|(address, (stake, _, _))| (*address, (*address, *address, *stake)))
                .collect::<IndexMap<_, _>>();

            // Construct the committee.
            let committee = Committee::<N>::new(0u64, members)?;
            (committee, bonded_balances)
        };

        // Ensure that the number of committee members is correct.
        ensure!(
            committee.members().len() == DEVELOPMENT_MODE_NUM_GENESIS_COMMITTEE_MEMBERS as usize,
            "Number of committee members {} does not match the expected number of members {DEVELOPMENT_MODE_NUM_GENESIS_COMMITTEE_MEMBERS}",
            committee.members().len()
        );

        // Calculate the public balance per validator.
        let remaining_balance = N::STARTING_SUPPLY.saturating_sub(committee.total_stake());
        let public_balance_per_validator =
            remaining_balance.saturating_div(DEVELOPMENT_MODE_NUM_GENESIS_COMMITTEE_MEMBERS as u64);

        // Construct the public balances with fairly equal distribution.
        let mut public_balances = development_private_keys
            .iter()
            .map(|private_key| {
                Ok((
                    Address::try_from(private_key)?,
                    public_balance_per_validator,
                ))
            })
            .collect::<Result<indexmap::IndexMap<_, _>>>()?;

        // If there is some leftover balance, add it to the 0-th validator.
        let leftover = remaining_balance.saturating_sub(
            public_balance_per_validator * DEVELOPMENT_MODE_NUM_GENESIS_COMMITTEE_MEMBERS as u64,
        );
        if leftover > 0 {
            let (_, balance) = public_balances.get_index_mut(0).unwrap();
            *balance += leftover;
        }

        // Check if the sum of committee stakes and public balances equals the total starting supply.
        let public_balances_sum: u64 = public_balances.values().copied().sum();
        if committee.total_stake() + public_balances_sum != N::STARTING_SUPPLY {
            bail!(
                "Sum of committee stakes and public balances does not equal total starting supply."
            );
        }

        // Construct the genesis block.
        load_or_compute_genesis(
            development_private_keys[0],
            committee,
            public_balances,
            bonded_balances,
            &mut rng,
        )
    }

    /// Returns the node type corresponding to the given configurations.
    #[rustfmt::skip]
    async fn parse_node<N: Network>(&mut self, shutdown: Arc<AtomicBool>) -> Result<Node<N>> {
        // Print the welcome.
        println!("{}", crate::helpers::welcome_message());

        // Parse the development configurations.
        self.parse_development()?;

        // Parse the genesis block.
        let genesis = self.parse_genesis::<N>()?;

        // Parse the private key of the node.
        let account = self.parse_private_key::<N>()?;

        // Parse the node IP.
        // Note: the `node` flag is an option to detect remote devnet testing.
        let node_ip = match self.node {
            Some(node_ip) => node_ip,
            None => SocketAddr::from_str("0.0.0.0:4130").unwrap(),
        };

        // Parse the REST IP.
        let rest_ip = self.rest.or_else(|| Some("0.0.0.0:3030".parse().unwrap()));

        // Print the Aleo address.
        println!("👛 Your Aleo address is {}.\n", account.address().to_string().bold());
        // Print the node type and network.
        println!(
            "🧭 Starting node on {} at {}.\n",
            N::NAME.bold(),
            node_ip.to_string().bold()
        );

        // If the node is running a REST server, print the REST IP and JWT.
        if let Some(rest_ip) = rest_ip {
            println!("🌐 Starting the REST server at {}.\n", rest_ip.to_string().bold());

            if let Ok(jwt_token) = snarkos_lite_node_rest::Claims::new(account.address()).to_jwt_string() {
                println!("🔑 Your one-time JWT token is {}\n", jwt_token.dimmed());
            }
        }

        // Initialize the metrics.
        if self.metrics {
            metrics::initialize_metrics(self.metrics_ip);
        }

        // Initialize the storage mode.
        let storage_mode = amareleo_storage_mode(self.network, self.storage.clone());

        // Determine whether to generate background transactions in dev mode.
        let dev_txs = !self.no_dev_txs;

        // // Initialize the node.
        Node::new_validator(node_ip, rest_ip, self.rest_rps, account, genesis, storage_mode, dev_txs, shutdown.clone()).await
    }

    /// Returns a runtime for the node.
    fn runtime() -> Runtime {
        // Retrieve the number of cores.
        let num_cores = num_cpus::get();

        // Initialize the number of tokio worker threads, max tokio blocking threads, and rayon cores.
        // Note: We intentionally set the number of tokio worker threads and number of rayon cores to be
        // more than the number of physical cores, because the node is expected to be I/O-bound.
        let (num_tokio_worker_threads, max_tokio_blocking_threads, num_rayon_cores_global) =
            (2 * num_cores, 512, num_cores);

        // Initialize the parallelization parameters.
        rayon::ThreadPoolBuilder::new()
            .stack_size(8 * 1024 * 1024)
            .num_threads(num_rayon_cores_global)
            .build_global()
            .unwrap();

        // Initialize the runtime configuration.
        runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(8 * 1024 * 1024)
            .worker_threads(num_tokio_worker_threads)
            .max_blocking_threads(max_tokio_blocking_threads)
            .build()
            .expect("Failed to initialize a runtime for the router")
    }
}

/// Loads or computes the genesis block.
fn load_or_compute_genesis<N: Network>(
    genesis_private_key: PrivateKey<N>,
    committee: Committee<N>,
    public_balances: indexmap::IndexMap<Address<N>, u64>,
    bonded_balances: indexmap::IndexMap<Address<N>, (Address<N>, Address<N>, u64)>,
    rng: &mut ChaChaRng,
) -> Result<Block<N>> {
    // Construct the preimage.
    let mut preimage = Vec::new();
    let raw_block0: &[u8];
    let raw_blockid: &str;

    // Input the network ID.
    preimage.extend(&N::ID.to_le_bytes());
    // Input the genesis coinbase target.
    preimage.extend(&to_bytes_le![N::GENESIS_COINBASE_TARGET]?);
    // Input the genesis proof target.
    preimage.extend(&to_bytes_le![N::GENESIS_PROOF_TARGET]?);

    // Input the genesis private key, committee, and public balances.
    preimage.extend(genesis_private_key.to_bytes_le()?);
    preimage.extend(committee.to_bytes_le()?);
    preimage.extend(&to_bytes_le![public_balances
        .iter()
        .collect::<Vec<(_, _)>>()]?);
    preimage.extend(&to_bytes_le![bonded_balances
        .iter()
        .flat_map(|(staker, (validator, withdrawal, amount))| to_bytes_le![
            staker, validator, withdrawal, amount
        ])
        .collect::<Vec<_>>()]?);

    // Input the parameters' metadata based on network
    match N::ID {
        snarkvm::console::network::MainnetV0::ID => {
            preimage
                .extend(snarkvm::parameters::mainnet::BondValidatorVerifier::METADATA.as_bytes());
            preimage.extend(snarkvm::parameters::mainnet::BondPublicVerifier::METADATA.as_bytes());
            preimage
                .extend(snarkvm::parameters::mainnet::UnbondPublicVerifier::METADATA.as_bytes());
            preimage.extend(
                snarkvm::parameters::mainnet::ClaimUnbondPublicVerifier::METADATA.as_bytes(),
            );
            preimage.extend(
                snarkvm::parameters::mainnet::SetValidatorStateVerifier::METADATA.as_bytes(),
            );
            preimage
                .extend(snarkvm::parameters::mainnet::TransferPrivateVerifier::METADATA.as_bytes());
            preimage
                .extend(snarkvm::parameters::mainnet::TransferPublicVerifier::METADATA.as_bytes());
            preimage.extend(
                snarkvm::parameters::mainnet::TransferPrivateToPublicVerifier::METADATA.as_bytes(),
            );
            preimage.extend(
                snarkvm::parameters::mainnet::TransferPublicToPrivateVerifier::METADATA.as_bytes(),
            );
            preimage.extend(snarkvm::parameters::mainnet::FeePrivateVerifier::METADATA.as_bytes());
            preimage.extend(snarkvm::parameters::mainnet::FeePublicVerifier::METADATA.as_bytes());
            preimage.extend(snarkvm::parameters::mainnet::InclusionVerifier::METADATA.as_bytes());
            raw_block0 = BLOCK0_MAINNET;
            raw_blockid = BLOCK0_MAINNET_ID;
        }
        snarkvm::console::network::TestnetV0::ID => {
            preimage
                .extend(snarkvm::parameters::testnet::BondValidatorVerifier::METADATA.as_bytes());
            preimage.extend(snarkvm::parameters::testnet::BondPublicVerifier::METADATA.as_bytes());
            preimage
                .extend(snarkvm::parameters::testnet::UnbondPublicVerifier::METADATA.as_bytes());
            preimage.extend(
                snarkvm::parameters::testnet::ClaimUnbondPublicVerifier::METADATA.as_bytes(),
            );
            preimage.extend(
                snarkvm::parameters::testnet::SetValidatorStateVerifier::METADATA.as_bytes(),
            );
            preimage
                .extend(snarkvm::parameters::testnet::TransferPrivateVerifier::METADATA.as_bytes());
            preimage
                .extend(snarkvm::parameters::testnet::TransferPublicVerifier::METADATA.as_bytes());
            preimage.extend(
                snarkvm::parameters::testnet::TransferPrivateToPublicVerifier::METADATA.as_bytes(),
            );
            preimage.extend(
                snarkvm::parameters::testnet::TransferPublicToPrivateVerifier::METADATA.as_bytes(),
            );
            preimage.extend(snarkvm::parameters::testnet::FeePrivateVerifier::METADATA.as_bytes());
            preimage.extend(snarkvm::parameters::testnet::FeePublicVerifier::METADATA.as_bytes());
            preimage.extend(snarkvm::parameters::testnet::InclusionVerifier::METADATA.as_bytes());
            raw_block0 = BLOCK0_TESTNET;
            raw_blockid = BLOCK0_TESTNET_ID;
        }
        snarkvm::console::network::CanaryV0::ID => {
            preimage
                .extend(snarkvm::parameters::canary::BondValidatorVerifier::METADATA.as_bytes());
            preimage.extend(snarkvm::parameters::canary::BondPublicVerifier::METADATA.as_bytes());
            preimage.extend(snarkvm::parameters::canary::UnbondPublicVerifier::METADATA.as_bytes());
            preimage.extend(
                snarkvm::parameters::canary::ClaimUnbondPublicVerifier::METADATA.as_bytes(),
            );
            preimage.extend(
                snarkvm::parameters::canary::SetValidatorStateVerifier::METADATA.as_bytes(),
            );
            preimage
                .extend(snarkvm::parameters::canary::TransferPrivateVerifier::METADATA.as_bytes());
            preimage
                .extend(snarkvm::parameters::canary::TransferPublicVerifier::METADATA.as_bytes());
            preimage.extend(
                snarkvm::parameters::canary::TransferPrivateToPublicVerifier::METADATA.as_bytes(),
            );
            preimage.extend(
                snarkvm::parameters::canary::TransferPublicToPrivateVerifier::METADATA.as_bytes(),
            );
            preimage.extend(snarkvm::parameters::canary::FeePrivateVerifier::METADATA.as_bytes());
            preimage.extend(snarkvm::parameters::canary::FeePublicVerifier::METADATA.as_bytes());
            preimage.extend(snarkvm::parameters::canary::InclusionVerifier::METADATA.as_bytes());
            raw_block0 = BLOCK0_CANARY;
            raw_blockid = BLOCK0_CANARY_ID;
        }
        _ => {
            // Unrecognized Network ID
            bail!("Unrecognized Network ID: {}", N::ID);
        }
    }

    // Initialize the hasher.
    let hasher = snarkvm::console::algorithms::BHP256::<N>::setup("aleo.dev.block")?;
    // Compute the hash.
    // NOTE: this is a fast-to-compute but *IMPERFECT* identifier for the genesis block.
    //       to know the actualy genesis block hash, you need to compute the block itself.
    let hash = hasher.hash(&preimage.to_bits_le())?.to_string();
    if hash == raw_blockid {
        println!("Loading Genesis Block from internal resource.");
        let block = Block::from_bytes_le(raw_block0)?;
        return Ok(block);
    }

    // A closure to load the block.
    let load_block = |file_path| -> Result<Block<N>> {
        // Attempts to load the genesis block file locally.
        let buffer = std::fs::read(file_path)?;
        // Return the genesis block.
        Block::from_bytes_le(&buffer)
    };

    // Construct the file path.
    let file_path = std::env::temp_dir().join(hash);
    // println!("Genesis Block file path: {}", file_path.display());

    // Check if the genesis block exists.
    if file_path.exists() {
        // If the block loads successfully, return it.
        if let Ok(block) = load_block(&file_path) {
            return Ok(block);
        }
    }

    /* Otherwise, compute the genesis block and store it. */

    // Initialize a new VM.
    let vm = VM::from(ConsensusStore::<N, ConsensusMemory<N>>::open(0u16)?)?;
    // Initialize the genesis block.
    let block = vm.genesis_quorum(
        &genesis_private_key,
        committee,
        public_balances,
        bonded_balances,
        rng,
    )?;
    // Write the genesis block to the file.
    std::fs::write(&file_path, block.to_bytes_le()?)?;
    // Return the genesis block.
    Ok(block)
}
