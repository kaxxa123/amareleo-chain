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

use snarkvm::{
    console::{algorithms::Hash, network::Network},
    prelude::{Result, ToBits, ToBytes, anyhow, bail},
};

use aleo_std::StorageMode;
use std::{net::SocketAddr, path::PathBuf};

const LEDGER_STD_DIR: &str = "amareleo-ledger";
const LEDGER_TMP_DIR: &str = "amareleo-tmp-ledger";
const LEDGER_DIR_TAG: &str = "-ledger-";
const PROPOSAL_CACHE_TAG: &str = "-proposal-cache-";
const LOG_STD_FILE: &str = "amareleo-chain";
const LOG_TMP_FILE: &str = "amareleo-chain-tmp";

pub const DEFAULT_FILE_TAG: &str = "0";

/// A string derived from the hash of the REST endpoint, giving us a unique filename per endpoint.
pub fn endpoint_file_tag<N: Network>(force_default: bool, endpoint: &SocketAddr) -> Result<String> {
    if force_default {
        return Ok(DEFAULT_FILE_TAG.to_string());
    }

    let default: SocketAddr = "0.0.0.0:3030".parse().unwrap();
    if *endpoint == default {
        return Ok(DEFAULT_FILE_TAG.to_string());
    }

    // Initialize the hasher.
    let hasher = snarkvm::console::algorithms::BHP256::<N>::setup("aleo.dev.block")?;
    let endpoint_str = endpoint.to_string();
    let bits = endpoint_str.to_bits_le();
    let hash = hasher.hash(&bits)?.to_bytes_le()?;
    let hash_str = hex::encode(hash);

    Ok(hash_str[..hash_str.len().min(8)].to_string())
}

/// Returns the ledger dir for the given base path
pub fn custom_ledger_dir(network: u16, keep_state: bool, end_point_tag: &str, base: PathBuf) -> PathBuf {
    let mut path = base.clone();
    path.push(format!(".{}-{network}-{end_point_tag}", if keep_state { LEDGER_STD_DIR } else { LEDGER_TMP_DIR }));
    path
}

/// Returns default ledger dir path
pub fn default_ledger_dir(network: u16, keep_state: bool, end_point_tag: &str) -> PathBuf {
    let path = match std::env::current_dir() {
        Ok(current_dir) => current_dir,
        _ => PathBuf::from(env!("CARGO_MANIFEST_DIR")),
    };
    custom_ledger_dir(network, keep_state, end_point_tag, path)
}

/// Wraps ledger dir in a StorageMode type
pub fn amareleo_storage_mode(ledger_path: PathBuf) -> StorageMode {
    StorageMode::Custom(ledger_path)
}

pub fn amareleo_log_file(network: u16, keep_state: bool, end_point_tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("{}-{network}-{end_point_tag}.log", if keep_state { LOG_STD_FILE } else { LOG_TMP_FILE }));
    path
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

#[cfg(test)]
mod tests {
    use super::*;

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
