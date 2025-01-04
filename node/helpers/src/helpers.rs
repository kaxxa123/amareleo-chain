use aleo_std::StorageMode;
use std::path::PathBuf;

/// Returns the path where a proposal cache file may be stored.
pub fn proposal_cache_path(network: u16, dev: Option<u16>) -> PathBuf {
    const PROPOSAL_CACHE_FILE_NAME: &str = "amareleo-proposal-cache";

    // Obtain the path to the ledger.
    let mut path = amareleo_ledger_dir(network);
    // Go to the folder right above the ledger.
    path.pop();
    // Append the proposal store's file name.
    match dev {
        Some(id) => path.push(format!(".{PROPOSAL_CACHE_FILE_NAME}-{network}-{id}")),
        None => path.push(format!("{PROPOSAL_CACHE_FILE_NAME}-{network}")),
    }

    path
}

pub fn amareleo_ledger_dir(network: u16) -> PathBuf {
    let mut path = match std::env::current_dir() {
        Ok(current_dir) => current_dir,
        _ => PathBuf::from(env!("CARGO_MANIFEST_DIR")),
    };
    path.push(format!(".amareleo-ledger-{}-0",network));
    path
}

pub fn amareleo_storage_mode(network: u16) -> StorageMode {
    StorageMode::Custom(amareleo_ledger_dir(network))
}
