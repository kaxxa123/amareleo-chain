use snarkos_lite_account::Account;
use snarkvm::prelude::{
    block::Block,
    store::ConsensusStorage,
    Ledger,
    Network,
};
use aleo_std::StorageMode;
use std::{
    net::SocketAddr,
    sync::{atomic::AtomicBool, Arc},
};
use anyhow::{Result, anyhow};


pub struct Validator<N: Network, C: ConsensusStorage<N>> {
    /// The ledger of the node.
    ledger: Ledger<N, C>,
}

impl<N: Network, C: ConsensusStorage<N>> Validator<N, C> {
    /// Initializes a new validator node.
    pub async fn new(
        node_ip: SocketAddr,
        bft_ip: Option<SocketAddr>,
        rest_ip: Option<SocketAddr>,
        rest_rps: u32,
        account: Account<N>,
        genesis: Block<N>,
        storage_mode: StorageMode,
        dev_txs: bool,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        Err(anyhow!("This is an error message"))
    }
}