use crate::traits::NodeInterface;

use aleo_std::StorageMode;
use anyhow::{anyhow, Result};
use snarkos_lite_account::Account;
use snarkvm::prelude::{block::Block, store::ConsensusStorage, Ledger, Network};
use std::{
    net::SocketAddr,
    sync::{atomic::AtomicBool, Arc},
};

#[derive(Clone)]
pub struct Validator<N: Network, C: ConsensusStorage<N>> {
    /// The ledger of the node.
    ledger: Ledger<N, C>,
    // /// The consensus module of the node.
    // consensus: Consensus<N>,
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
        // Initialize the signal handler.
        let signal_node = Self::handle_signals(shutdown.clone());

        // Initialize the ledger.
        let ledger: Ledger<N, C> = Ledger::load(genesis, storage_mode.clone())?;

        Err(anyhow!("This is an error message"))
    }

    /// Returns the ledger.
    pub fn ledger(&self) -> &Ledger<N, C> {
        &self.ledger
    }
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> NodeInterface<N> for Validator<N, C> {
    /// Shuts down the node.
    async fn shut_down(&self) {
        info!("Shutting down...");

        // // Shut down the node.
        // trace!("Shutting down the node...");
        // self.shutdown
        //     .store(true, std::sync::atomic::Ordering::Release);

        // // Abort the tasks.
        // trace!("Shutting down the validator...");
        // self.handles.lock().iter().for_each(|handle| handle.abort());

        // // Shut down the router.
        // self.router.shut_down().await;

        // // Shut down consensus.
        // trace!("Shutting down consensus...");
        // self.consensus.shut_down().await;

        info!("Node has shut down.");
    }
}
