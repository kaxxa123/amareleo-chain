// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at:

// http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::{Result, bail, ensure};
use indexmap::IndexMap;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaChaRng;

use snarkvm::{
    console::{
        account::{Address, PrivateKey},
        algorithms::Hash,
        network::Network,
    },
    ledger::{
        block::Block,
        committee::{Committee, MIN_VALIDATOR_STAKE},
        store::{
            ConsensusStore,
            helpers::{memory::ConsensusMemory, rocksdb::ConsensusDB},
        },
    },
    prelude::{FromBytes, ToBits, ToBytes},
    synthesizer::VM,
    utilities::to_bytes_le,
};

use crate::AmareleoLog;

use amareleo_chain_account::Account;
use amareleo_chain_resources::{
    BLOCK0_CANARY,
    BLOCK0_CANARY_ID,
    BLOCK0_MAINNET,
    BLOCK0_MAINNET_ID,
    BLOCK0_TESTNET,
    BLOCK0_TESTNET_ID,
};
use amareleo_chain_tracing::{TracingHandler, initialize_tracing};
use amareleo_node::Validator;
use amareleo_node_bft::{
    DEVELOPMENT_MODE_NUM_GENESIS_COMMITTEE_MEMBERS,
    DEVELOPMENT_MODE_RNG_SEED,
    helpers::{
        amareleo_log_file,
        amareleo_storage_mode,
        custom_ledger_dir,
        default_ledger_dir,
        endpoint_file_tag,
        proposal_cache_path,
    },
};

/// Amareleo node object, for creating and managing a node instance
#[derive(Clone)]
pub struct AmareleoApi<N: Network> {
    rest_ip: SocketAddr,
    rest_rps: u32,
    keep_state: bool,
    ledger_base_path: Option<PathBuf>,
    ledger_default_naming: bool,
    log_mode: AmareleoLog,
    log_trace: Option<TracingHandler>,
    verbosity: u8,
    shutdown: Arc<AtomicBool>,
    validator: Option<Arc<Validator<N, ConsensusDB<N>>>>,
}

impl<N: Network> Default for AmareleoApi<N> {
    /// Create a node with default configuration.
    /// Listenning to port 3030, with 10 requests per second limit.
    /// No ledger state retenttion, default ledger folder naming, no logging.
    fn default() -> Self {
        Self {
            rest_ip: "0.0.0.0:3030".parse().unwrap(),
            rest_rps: 10u32,
            keep_state: false,
            ledger_base_path: None,
            ledger_default_naming: false,
            log_mode: AmareleoLog::None,
            log_trace: None,
            verbosity: 1u8,
            shutdown: Default::default(),
            validator: None,
        }
    }
}

// AmareleoApi configuration setters
impl<N: Network> AmareleoApi<N> {
    /// Configure REST server IP, port, and requests per second limit
    /// ip_port: SocketAddr - IP and port to listen to
    /// rps: u32 - requests per second limit
    pub fn cfg_rest(&mut self, ip_port: SocketAddr, rps: u32) -> &mut Self {
        if !self.is_started() {
            self.rest_ip = ip_port;
            self.rest_rps = rps;
        }

        self
    }

    /// Configure ledger storage properties
    /// keep_state: bool - keep ledger state between restarts
    /// base_path: Option<PathBuf> - custom ledger folder path
    /// default_naming: bool - use default ledger folder naming, instead of deriving unique names from the rest IP:port configuration.
    pub fn cfg_ledger(&mut self, keep_state: bool, base_path: Option<PathBuf>, default_naming: bool) -> &mut Self {
        if !self.is_started() {
            self.keep_state = keep_state;
            self.ledger_base_path = base_path;
            self.ledger_default_naming = default_naming;
        }

        self
    }

    /// Configure file logging path and verbosity level
    /// log_file: Option<PathBuf> - custom log file path, or None for default path
    /// verbosity: u8 - log verbosity level between 0 and 4, where 0 is the least verbose
    pub fn cfg_file_log(&mut self, log_file: Option<PathBuf>, verbosity: u8) -> &mut Self {
        if !self.is_started() {
            self.log_mode = AmareleoLog::File(log_file);
            self.verbosity = verbosity;
        }

        self
    }

    /// Configure custom logging
    /// tracing: TracingHandler - custom tracing subscriber
    pub fn cfg_custom_log(&mut self, tracing: TracingHandler) -> &mut Self {
        if !self.is_started() {
            self.log_mode = AmareleoLog::Custom(tracing);
        }

        self
    }

    /// Disable logging
    pub fn cfg_no_log(&mut self) -> &mut Self {
        if !self.is_started() {
            self.log_mode = AmareleoLog::None;
        }

        self
    }
}

// AmareleoApi public getters
impl<N: Network> AmareleoApi<N> {
    /// Get well known development-mode node account, use the public and private credits to pay for transaction fees.
    pub fn get_node_account() -> Result<Account<N>> {
        Account::try_from({
            // Initialize the (fixed) RNG.
            let mut rng = ChaChaRng::seed_from_u64(DEVELOPMENT_MODE_RNG_SEED);
            PrivateKey::<N>::new(&mut rng)?
        })
    }

    /// Get resultant log file path, based on current configuration.
    /// Fails if file logging is not configured.
    pub fn get_log_file(&self) -> Result<PathBuf> {
        if !self.log_mode.is_file() {
            bail!("Logging to file is disabled!");
        }

        let log_option = self.log_mode.get_path();
        let log_path = match &log_option {
            Some(path) => path.clone(),
            None => {
                let end_point_tag = endpoint_file_tag::<N>(false, &self.rest_ip)?;
                amareleo_log_file(N::ID, self.keep_state, &end_point_tag)
            }
        };

        Ok(log_path)
    }

    /// Get resultant ledger folder path, based on current configuration.
    pub fn get_ledger_folder(&self) -> Result<PathBuf> {
        let tag = endpoint_file_tag::<N>(self.ledger_default_naming, &self.rest_ip)?;
        let ledger_path = match &self.ledger_base_path {
            Some(base_path) => custom_ledger_dir(N::ID, self.keep_state, &tag, base_path.clone()),
            None => default_ledger_dir(N::ID, self.keep_state, &tag),
        };

        Ok(ledger_path.to_path_buf())
    }

    /// Check if the node is started
    pub fn is_started(&self) -> bool {
        self.validator.is_some()
    }
}

// AmareleoApi operations for public consumption
impl<N: Network> AmareleoApi<N> {
    /// Start a node instance
    pub async fn start(&mut self) -> Result<()> {
        if self.is_started() {
            bail!("Node already started");
        }

        let genesis = Self::get_genesis()?;
        let account = Self::get_node_account()?;
        let ledger_path = self.get_ledger_folder()?;
        let storage_mode = amareleo_storage_mode(ledger_path.clone());
        if !self.keep_state {
            // Clean the temporary ledger.
            Self::clean_tmp_ledger(ledger_path)?;
        }

        self.trace_init()?;

        // Initialize the validator.
        let validator: Validator<N, ConsensusDB<N>> = Validator::new(
            self.rest_ip,
            self.rest_rps,
            account,
            genesis,
            self.keep_state,
            storage_mode,
            self.log_trace.clone(),
            self.shutdown.clone(),
        )
        .await?;

        self.validator = Some(Arc::new(validator));

        Ok(())
    }

    /// Stop the node instance
    pub async fn end(&mut self) {
        if let Some(validator) = &self.validator {
            validator.shut_down().await;
            self.validator = None;
        }
    }
}

// AmareleoApi private non-static helpers
impl<N: Network> AmareleoApi<N> {
    /// Initialze logging
    fn trace_init(&mut self) -> Result<()> {
        // Determine the effective TraceHandler
        self.log_trace = match &self.log_mode {
            AmareleoLog::File(_) => Some(initialize_tracing(self.verbosity, self.get_log_file()?)?),
            AmareleoLog::Custom(tracing) => Some(tracing.clone()),
            AmareleoLog::None => None,
        };

        Ok(())
    }
}

// AmareleoApi private static helpers
impl<N: Network> AmareleoApi<N> {
    // Cleans the temporary ledger
    fn clean_tmp_ledger(ledger_path: PathBuf) -> Result<()> {
        // Remove the current proposal cache file, if it exists.
        let storage_mode = amareleo_storage_mode(ledger_path.clone());
        let cache_path = proposal_cache_path(&storage_mode)?;
        if cache_path.exists() {
            if let Err(err) = std::fs::remove_file(&cache_path) {
                bail!("Failed on removing proposal cache file at {}: {err}", cache_path.display());
            }
        }

        // Remove ledger
        if ledger_path.exists() {
            if let Err(err) = std::fs::remove_dir_all(&ledger_path) {
                bail!("Failed on removing ledger folder {}: {err}", ledger_path.display());
            }
        }

        Ok(())
    }

    // Returns genesis block for the development mode node.
    fn get_genesis() -> Result<Block<N>> {
        // Initialize the (fixed) RNG.
        let mut rng = ChaChaRng::seed_from_u64(DEVELOPMENT_MODE_RNG_SEED);

        // Initialize the development private keys.
        let development_private_keys = (0..DEVELOPMENT_MODE_NUM_GENESIS_COMMITTEE_MEMBERS)
            .map(|_| PrivateKey::<N>::new(&mut rng))
            .collect::<Result<Vec<_>>>()?;
        // Initialize the development addresses.
        let development_addresses =
            development_private_keys.iter().map(Address::<N>::try_from).collect::<Result<Vec<_>>>()?;

        let (committee, bonded_balances) = {
            // Calculate the committee stake per member.
            let stake_per_member = N::STARTING_SUPPLY
                .saturating_div(2)
                .saturating_div(DEVELOPMENT_MODE_NUM_GENESIS_COMMITTEE_MEMBERS as u64);
            ensure!(stake_per_member >= MIN_VALIDATOR_STAKE, "Committee stake per member is too low");

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
            .map(|private_key| Ok((Address::try_from(private_key)?, public_balance_per_validator)))
            .collect::<Result<IndexMap<_, _>>>()?;

        // If there is some leftover balance, add it to the 0-th validator.
        let leftover = remaining_balance
            .saturating_sub(public_balance_per_validator * DEVELOPMENT_MODE_NUM_GENESIS_COMMITTEE_MEMBERS as u64);
        if leftover > 0 {
            let (_, balance) = public_balances.get_index_mut(0).unwrap();
            *balance += leftover;
        }

        // Check if the sum of committee stakes and public balances equals the total starting supply.
        let public_balances_sum: u64 = public_balances.values().copied().sum();
        if committee.total_stake() + public_balances_sum != N::STARTING_SUPPLY {
            bail!("Sum of committee stakes and public balances does not equal total starting supply.");
        }

        // Construct the genesis block.
        Self::load_or_compute_genesis(
            development_private_keys[0],
            committee,
            public_balances,
            bonded_balances,
            &mut rng,
        )
    }

    // Loads or computes the genesis block.
    fn load_or_compute_genesis(
        genesis_private_key: PrivateKey<N>,
        committee: Committee<N>,
        public_balances: IndexMap<Address<N>, u64>,
        bonded_balances: IndexMap<Address<N>, (Address<N>, Address<N>, u64)>,
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
        preimage.extend(&to_bytes_le![public_balances.iter().collect::<Vec<(_, _)>>()]?);
        preimage.extend(&to_bytes_le![
            bonded_balances
                .iter()
                .flat_map(|(staker, (validator, withdrawal, amount))| to_bytes_le![
                    staker, validator, withdrawal, amount
                ])
                .collect::<Vec<_>>()
        ]?);

        // Input the parameters' metadata based on network
        match N::ID {
            snarkvm::console::network::MainnetV0::ID => {
                preimage.extend(snarkvm::parameters::mainnet::BondValidatorVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::mainnet::BondPublicVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::mainnet::UnbondPublicVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::mainnet::ClaimUnbondPublicVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::mainnet::SetValidatorStateVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::mainnet::TransferPrivateVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::mainnet::TransferPublicVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::mainnet::TransferPrivateToPublicVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::mainnet::TransferPublicToPrivateVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::mainnet::FeePrivateVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::mainnet::FeePublicVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::mainnet::InclusionVerifier::METADATA.as_bytes());
                raw_block0 = BLOCK0_MAINNET;
                raw_blockid = BLOCK0_MAINNET_ID;
            }
            snarkvm::console::network::TestnetV0::ID => {
                preimage.extend(snarkvm::parameters::testnet::BondValidatorVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::testnet::BondPublicVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::testnet::UnbondPublicVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::testnet::ClaimUnbondPublicVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::testnet::SetValidatorStateVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::testnet::TransferPrivateVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::testnet::TransferPublicVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::testnet::TransferPrivateToPublicVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::testnet::TransferPublicToPrivateVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::testnet::FeePrivateVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::testnet::FeePublicVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::testnet::InclusionVerifier::METADATA.as_bytes());
                raw_block0 = BLOCK0_TESTNET;
                raw_blockid = BLOCK0_TESTNET_ID;
            }
            snarkvm::console::network::CanaryV0::ID => {
                preimage.extend(snarkvm::parameters::canary::BondValidatorVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::canary::BondPublicVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::canary::UnbondPublicVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::canary::ClaimUnbondPublicVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::canary::SetValidatorStateVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::canary::TransferPrivateVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::canary::TransferPublicVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::canary::TransferPrivateToPublicVerifier::METADATA.as_bytes());
                preimage.extend(snarkvm::parameters::canary::TransferPublicToPrivateVerifier::METADATA.as_bytes());
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
        // NOTE: this is a fast-to-compute but *IMPERFECT* identifier for the genesis block;
        //       to know the actual genesis block hash, you need to compute the block itself.
        let hash = hasher.hash(&preimage.to_bits_le())?.to_string();
        if hash == raw_blockid {
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
        let block = vm.genesis_quorum(&genesis_private_key, committee, public_balances, bonded_balances, rng)?;
        // Write the genesis block to the file.
        std::fs::write(&file_path, block.to_bytes_le()?)?;
        // Return the genesis block.
        Ok(block)
    }
}
