// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at:

// http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::path::PathBuf;

use anyhow::{Result, bail, ensure};
use indexmap::IndexMap;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaChaRng;

use snarkvm::{
    console::{
        account::{Address, PrivateKey},
        algorithms::Hash,
        network::{CanaryV0, MainnetV0, Network, TestnetV0},
    },
    ledger::{
        block::Block,
        committee::{Committee, MIN_VALIDATOR_STAKE},
        store::{ConsensusStore, helpers::memory::ConsensusMemory},
    },
    prelude::{FromBytes, ToBits, ToBytes},
    synthesizer::VM,
    utilities::to_bytes_le,
};

use amareleo_chain_account::Account;
use amareleo_chain_resources::{
    BLOCK0_CANARY,
    BLOCK0_CANARY_ID,
    BLOCK0_MAINNET,
    BLOCK0_MAINNET_ID,
    BLOCK0_TESTNET,
    BLOCK0_TESTNET_ID,
};
use amareleo_node_bft::{
    DEVELOPMENT_MODE_NUM_GENESIS_COMMITTEE_MEMBERS,
    DEVELOPMENT_MODE_RNG_SEED,
    helpers::{amareleo_storage_mode, proposal_cache_path},
};

/// Get the fixed node account, loaded with public
/// and private credits to pay for transaction fees.
///
/// # Returns
///
/// * `Result<Account<N>>` - the node primary account
pub fn get_node_account<N: Network>() -> Result<Account<N>> {
    Account::try_from({
        // Initialize the (fixed) RNG.
        let mut rng = ChaChaRng::seed_from_u64(DEVELOPMENT_MODE_RNG_SEED);
        PrivateKey::<N>::new(&mut rng)?
    })
}

/// Get the genesis block.
///
/// # Returns
///
/// * `Result<Block<N>>` - genesis block
pub fn get_genesis<N: Network>() -> Result<Block<N>> {
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
        let stake_per_member =
            N::STARTING_SUPPLY.saturating_div(2).saturating_div(DEVELOPMENT_MODE_NUM_GENESIS_COMMITTEE_MEMBERS as u64);
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
    load_or_compute_genesis::<N>(development_private_keys[0], committee, public_balances, bonded_balances, &mut rng)
}

// Check if network id is valid
pub(crate) fn is_valid_network_id(network_id: u16) -> bool {
    network_id == CanaryV0::ID || network_id == TestnetV0::ID || network_id == MainnetV0::ID
}

// Cleans the temporary ledger
pub(crate) fn clean_tmp_ledger(ledger_path: PathBuf) -> Result<()> {
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

// Loads or computes the genesis block.
pub(crate) fn load_or_compute_genesis<N: Network>(
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
            .flat_map(|(staker, (validator, withdrawal, amount))| to_bytes_le![staker, validator, withdrawal, amount])
            .collect::<Vec<_>>()
    ]?);

    // Input the parameters' metadata based on network
    match N::ID {
        MainnetV0::ID => {
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
        TestnetV0::ID => {
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
        CanaryV0::ID => {
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
