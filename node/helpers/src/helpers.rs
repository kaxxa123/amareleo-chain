use std::path::PathBuf;
use aleo_std::{aleo_ledger_dir, StorageMode};

/// Returns the path where a proposal cache file may be stored.
pub fn proposal_cache_path(network: u16, dev: Option<u16>) -> PathBuf {
    const PROPOSAL_CACHE_FILE_NAME: &str = "current-proposal-cache";

    // Obtain the path to the ledger.
    let mut path = aleo_ledger_dir(network, StorageMode::from(dev));
    // Go to the folder right above the ledger.
    path.pop();
    // Append the proposal store's file name.
    match dev {
        Some(id) => path.push(&format!(".{PROPOSAL_CACHE_FILE_NAME}-{}-{}", network, id)),
        None => path.push(&format!("{PROPOSAL_CACHE_FILE_NAME}-{}", network)),
    }

    path
}
