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

use crate::helpers::{Proposal, SignedProposals};

use snarkvm::{
    console::{account::Address, network::Network, program::SUBDAG_CERTIFICATES_DEPTH},
    ledger::narwhal::BatchCertificate,
    prelude::{FromBytes, IoResult, Read, Result, ToBytes, Write, anyhow, bail, error},
};

use aleo_std::StorageMode;
use indexmap::IndexSet;
use std::{fs, path::PathBuf};

const LEDGER_STD_DIR: &str = "amareleo-ledger";
const LEDGER_TMP_DIR: &str = "amareleo-tmp-ledger";
const LEDGER_DIR_TAG: &str = "-ledger-";
const PROPOSAL_CACHE_TAG: &str = "-proposal-cache-";

/// Returns the ledger dir for the given base path
pub fn custom_ledger_dir(network: u16, keep_state: bool, base: PathBuf) -> PathBuf {
    let mut path = base.clone();
    path.push(format!(".{}-{network}-0", if keep_state { LEDGER_STD_DIR } else { LEDGER_TMP_DIR }));
    path
}

/// Returns default ledger dir path
pub fn amareleo_ledger_dir(network: u16, keep_state: bool) -> PathBuf {
    let path = match std::env::current_dir() {
        Ok(current_dir) => current_dir,
        _ => PathBuf::from(env!("CARGO_MANIFEST_DIR")),
    };
    custom_ledger_dir(network, keep_state, path)
}

/// Wraps ledger dir in a StorageMode type
pub fn amareleo_storage_mode(ledger_path: PathBuf) -> StorageMode {
    StorageMode::Custom(ledger_path)
}

/// Returns the path where a proposal cache file may be stored.
pub fn proposal_cache_path(storage_mode: &StorageMode) -> Result<PathBuf> {
    // Obtain the path to the ledger.
    let mut path = match &storage_mode {
        StorageMode::Custom(path) => path.clone(),
        _ => bail!("Failed: Custom StorageMode expected!"),
    };

    // Get the ledger folder name
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("Failed: Cannot extract ledger folder name"))?;

    // Swap folder name with cache filename
    let filename = if filename.contains(LEDGER_DIR_TAG) {
        filename.replace(LEDGER_DIR_TAG, PROPOSAL_CACHE_TAG)
    } else {
        bail!("Failed: Unexpected ledger folder name!")
    };

    // Swap ledger folder name with cahce filename
    path.pop();
    path.push(filename);

    Ok(path)
}

/// A helper type for the cache of proposal and signed proposals.
#[derive(Debug, PartialEq, Eq)]
pub struct ProposalCache<N: Network> {
    /// The latest round this node was on prior to the reboot.
    latest_round: u64,
    /// The latest proposal this node has created.
    proposal: Option<Proposal<N>>,
    /// The signed proposals this node has received.
    signed_proposals: SignedProposals<N>,
    /// The pending certificates in storage that have not been included in the ledger.
    pending_certificates: IndexSet<BatchCertificate<N>>,
}

impl<N: Network> ProposalCache<N> {
    /// Initializes a new instance of the proposal cache.
    pub fn new(
        latest_round: u64,
        proposal: Option<Proposal<N>>,
        signed_proposals: SignedProposals<N>,
        pending_certificates: IndexSet<BatchCertificate<N>>,
    ) -> Self {
        Self { latest_round, proposal, signed_proposals, pending_certificates }
    }

    /// Ensure that the proposal and every signed proposal is associated with the `expected_signer`.
    pub fn is_valid(&self, expected_signer: Address<N>) -> bool {
        self.proposal
            .as_ref()
            .map(|proposal| {
                proposal.batch_header().author() == expected_signer && self.latest_round == proposal.round()
            })
            .unwrap_or(true)
            && self.signed_proposals.is_valid(expected_signer)
    }

    /// Returns `true` if a proposal cache exists for the given network.
    pub fn exists(storage_mode: &StorageMode) -> bool {
        proposal_cache_path(storage_mode).map_or(false, |path| path.exists())
    }

    /// Load the proposal cache from the file system and ensure that the proposal cache is valid.
    pub fn load(expected_signer: Address<N>, storage_mode: &StorageMode) -> Result<Self> {
        // Construct the proposal cache file system path.
        let path = proposal_cache_path(storage_mode)?;

        // Deserialize the proposal cache from the file system.
        let proposal_cache = match fs::read(&path) {
            Ok(bytes) => match Self::from_bytes_le(&bytes) {
                Ok(proposal_cache) => proposal_cache,
                Err(_) => bail!("Couldn't deserialize the proposal stored at {}", path.display()),
            },
            Err(_) => bail!("Couldn't read the proposal stored at {}", path.display()),
        };

        // Ensure the proposal cache is valid.
        if !proposal_cache.is_valid(expected_signer) {
            bail!("The proposal cache is invalid for the given address {expected_signer}");
        }

        info!("Loaded the proposal cache from {} at round {}", path.display(), proposal_cache.latest_round);

        Ok(proposal_cache)
    }

    /// Store the proposal cache to the file system.
    pub fn store(&self, storage_mode: &StorageMode) -> Result<()> {
        let path = proposal_cache_path(storage_mode)?;
        info!("Storing the proposal cache to {}...", path.display());

        // Serialize the proposal cache.
        let bytes = self.to_bytes_le()?;
        // Store the proposal cache to the file system.
        fs::write(&path, bytes)
            .map_err(|err| anyhow!("Couldn't write the proposal cache to {} - {err}", path.display()))?;

        Ok(())
    }

    /// Returns the latest round, proposal, signed proposals, and pending certificates.
    pub fn into(self) -> (u64, Option<Proposal<N>>, SignedProposals<N>, IndexSet<BatchCertificate<N>>) {
        (self.latest_round, self.proposal, self.signed_proposals, self.pending_certificates)
    }
}

impl<N: Network> ToBytes for ProposalCache<N> {
    fn write_le<W: Write>(&self, mut writer: W) -> IoResult<()> {
        // Serialize the `latest_round`.
        self.latest_round.write_le(&mut writer)?;
        // Serialize the `proposal`.
        self.proposal.is_some().write_le(&mut writer)?;
        if let Some(proposal) = &self.proposal {
            proposal.write_le(&mut writer)?;
        }
        // Serialize the `signed_proposals`.
        self.signed_proposals.write_le(&mut writer)?;
        // Write the number of pending certificates.
        u32::try_from(self.pending_certificates.len()).map_err(error)?.write_le(&mut writer)?;
        // Serialize the pending certificates.
        for certificate in &self.pending_certificates {
            certificate.write_le(&mut writer)?;
        }

        Ok(())
    }
}

impl<N: Network> FromBytes for ProposalCache<N> {
    fn read_le<R: Read>(mut reader: R) -> IoResult<Self> {
        // Deserialize `latest_round`.
        let latest_round = u64::read_le(&mut reader)?;
        // Deserialize `proposal`.
        let has_proposal: bool = FromBytes::read_le(&mut reader)?;
        let proposal = match has_proposal {
            true => Some(Proposal::read_le(&mut reader)?),
            false => None,
        };
        // Deserialize `signed_proposals`.
        let signed_proposals = SignedProposals::read_le(&mut reader)?;
        // Read the number of pending certificates.
        let num_certificates = u32::read_le(&mut reader)?;
        // Ensure the number of certificates is within bounds.
        if num_certificates > 2u32.saturating_pow(SUBDAG_CERTIFICATES_DEPTH as u32) {
            return Err(error(format!(
                "Number of certificates ({num_certificates}) exceeds the maximum ({})",
                2u32.saturating_pow(SUBDAG_CERTIFICATES_DEPTH as u32)
            )));
        };
        // Deserialize the pending certificates.
        let pending_certificates =
            (0..num_certificates).map(|_| BatchCertificate::read_le(&mut reader)).collect::<IoResult<IndexSet<_>>>()?;

        Ok(Self::new(latest_round, proposal, signed_proposals, pending_certificates))
    }
}

impl<N: Network> Default for ProposalCache<N> {
    /// Initializes a new instance of the proposal cache.
    fn default() -> Self {
        Self::new(0, None, Default::default(), Default::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helpers::{proposal::tests::sample_proposal, signed_proposals::tests::sample_signed_proposals};
    use snarkvm::{
        console::{account::PrivateKey, network::MainnetV0},
        ledger::narwhal::batch_certificate::test_helpers::sample_batch_certificates,
        utilities::TestRng,
    };
    use std::path::PathBuf;

    type CurrentNetwork = MainnetV0;

    const ITERATIONS: usize = 100;

    pub(crate) fn sample_proposal_cache(
        signer: &PrivateKey<CurrentNetwork>,
        rng: &mut TestRng,
    ) -> ProposalCache<CurrentNetwork> {
        let proposal = sample_proposal(rng);
        let signed_proposals = sample_signed_proposals(signer, rng);
        let round = proposal.round();
        let pending_certificates = sample_batch_certificates(rng);

        ProposalCache::new(round, Some(proposal), signed_proposals, pending_certificates)
    }

    #[test]
    fn test_bytes() {
        let rng = &mut TestRng::default();
        let singer_private_key = PrivateKey::<CurrentNetwork>::new(rng).unwrap();

        for _ in 0..ITERATIONS {
            let expected = sample_proposal_cache(&singer_private_key, rng);
            // Check the byte representation.
            let expected_bytes = expected.to_bytes_le().unwrap();
            assert_eq!(expected, ProposalCache::read_le(&expected_bytes[..]).unwrap());
        }
    }

    #[test]
    fn test_proposal_cache_path_from_invalid_ledger() {
        let invalid = PathBuf::from("~/invalid");
        let store_mode = amareleo_storage_mode(invalid);

        let res = proposal_cache_path(&store_mode);
        assert!(res.is_err(), "Expected an error, but got Ok.");
    }

    #[test]
    fn test_proposal_cache_path_from_invalid_store_mode() {
        let store_mode = StorageMode::Development(0);
        let res = proposal_cache_path(&store_mode);
        assert!(res.is_err(), "Expected an error, but got Ok.");
    }
}
