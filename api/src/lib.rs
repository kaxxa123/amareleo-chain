use std::{
    net::SocketAddr,
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::Result;

use snarkvm::{
    console::network::Network,
    ledger::{block::Block, store::ConsensusStorage},
};

use aleo_std::StorageMode;

use amareleo_chain_account::Account;
use amareleo_node::Validator;

pub async fn node_api<N: Network, C: ConsensusStorage<N>>(
    rest_ip: Option<SocketAddr>,
    rest_rps: u32,
    account: Account<N>,
    genesis: Block<N>,
    keep_state: bool,
    storage_mode: StorageMode,
    shutdown: Arc<AtomicBool>,
) -> Result<Validator<N, C>> {
    Validator::new(rest_ip, rest_rps, account, genesis, keep_state, storage_mode, shutdown.clone()).await
}
