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

mod router;

use crate::traits::NodeInterface;
use snarkos_lite_account::Account;
use snarkos_lite_node_bft::{
    helpers::init_primary_channels, ledger_service::CoreLedgerService, spawn_blocking,
};
use snarkos_lite_node_consensus::Consensus;
use snarkos_lite_node_rest::Rest;
use snarkos_lite_node_router::{
    messages::{NodeType, PuzzleResponse, UnconfirmedSolution, UnconfirmedTransaction},
    Heartbeat, Inbound, Outbound, Router, Routing,
};
use snarkos_lite_node_sync::{BlockSync, BlockSyncMode};
use snarkos_lite_node_tcp::{
    protocols::{Disconnect, Handshake, OnConnect, Reading, Writing},
    P2P,
};
use snarkvm::prelude::{block::Block, store::ConsensusStorage, Ledger, Network};

use aleo_std::StorageMode;
use anyhow::Result;
use core::future::Future;
use parking_lot::Mutex;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};
use tokio::task::JoinHandle;

/// A validator is a full node, capable of validating blocks.
#[derive(Clone)]
pub struct Validator<N: Network, C: ConsensusStorage<N>> {
    /// The ledger of the node.
    ledger: Ledger<N, C>,
    /// The consensus module of the node.
    consensus: Consensus<N>,
    /// The router of the node.
    router: Router<N>,
    /// The REST server of the node.
    rest: Option<Rest<N, C, Self>>,
    /// The sync module.
    sync: BlockSync<N>,
    /// The spawned handles.
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
    /// The shutdown signal.
    shutdown: Arc<AtomicBool>,
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
        let ledger = Ledger::load(genesis, storage_mode.clone())?;

        // Initialize the ledger service.
        let ledger_service = Arc::new(CoreLedgerService::new(ledger.clone(), shutdown.clone()));

        // Initialize the consensus.
        let trusted_validators: [SocketAddr; 1] = [SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            5001,
        )];
        let mut consensus = Consensus::new(
            account.clone(),
            ledger_service.clone(),
            bft_ip,
            &trusted_validators,
            storage_mode.clone(),
        )?;
        // Initialize the primary channels.
        let (primary_sender, primary_receiver) = init_primary_channels::<N>();
        // Start the consensus.
        consensus.run(primary_sender, primary_receiver).await?;

        // Initialize the node router.
        let trusted_peers: [SocketAddr; 0] = [];
        let router = Router::new(
            node_ip,
            NodeType::Validator,
            account,
            &trusted_peers,
            Self::MAXIMUM_NUMBER_OF_PEERS as u16,
            false,
            false,
            true,
        )
        .await?;

        // Initialize the sync module.
        let sync = BlockSync::new(BlockSyncMode::Gateway, ledger_service);

        // Initialize the node.
        let mut node = Self {
            ledger: ledger.clone(),
            consensus: consensus.clone(),
            router,
            rest: None,
            sync,
            handles: Default::default(),
            shutdown,
        };
        // Initialize the transaction pool.
        node.initialize_transaction_pool(dev_txs)?;

        // Initialize the REST server.
        if let Some(rest_ip) = rest_ip {
            node.rest = Some(
                Rest::start(
                    rest_ip,
                    rest_rps,
                    Some(consensus),
                    ledger.clone(),
                    Arc::new(node.clone()),
                )
                .await?,
            );
        }
        // Initialize the routing.
        node.initialize_routing().await;

        // Initialize the notification message loop.
        node.handles
            .lock()
            .push(crate::start_notification_message_loop());

        // Pass the node to the signal handler.
        let _ = signal_node.set(node.clone());
        // Return the node.
        Ok(node)
    }

    /// Returns the ledger.
    pub fn ledger(&self) -> &Ledger<N, C> {
        &self.ledger
    }

    /// Returns the REST server.
    pub fn rest(&self) -> &Option<Rest<N, C, Self>> {
        &self.rest
    }
}

impl<N: Network, C: ConsensusStorage<N>> Validator<N, C> {
    /// Initialize the transaction pool.
    fn initialize_transaction_pool(&self, dev_txs: bool) -> Result<()> {
        use snarkvm::console::{
            program::{Identifier, Literal, ProgramID, Value},
            types::U64,
        };
        use std::str::FromStr;

        // Initialize the locator.
        let locator = (
            ProgramID::from_str("credits.aleo")?,
            Identifier::from_str("transfer_public")?,
        );

        // Determine whether to start the loop.
        if !dev_txs {
            return Ok(());
        }

        let self_ = self.clone();
        self.spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            info!("Starting transaction pool...");

            // Start the transaction loop.
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;

                // Prepare the inputs.
                let inputs = [
                    Value::from(Literal::Address(self_.address())),
                    Value::from(Literal::U64(U64::new(1))),
                ];
                // Execute the transaction.
                let self__ = self_.clone();
                let transaction = match spawn_blocking!(self__.ledger.vm().execute(
                    self__.private_key(),
                    locator,
                    inputs.into_iter(),
                    None,
                    10_000,
                    None,
                    &mut rand::thread_rng(),
                )) {
                    Ok(transaction) => transaction,
                    Err(error) => {
                        error!("Transaction pool encountered an execution error - {error}");
                        continue;
                    }
                };
                // Broadcast the transaction.
                if self_
                    .unconfirmed_transaction(
                        self_.router.local_ip(),
                        UnconfirmedTransaction::from(transaction.clone()),
                        transaction.clone(),
                    )
                    .await
                {
                    info!("Transaction pool broadcasted the transaction");
                }
            }
        });
        Ok(())
    }

    /// Spawns a task with the given future; it should only be used for long-running tasks.
    pub fn spawn<T: Future<Output = ()> + Send + 'static>(&self, future: T) {
        self.handles.lock().push(tokio::spawn(future));
    }
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> NodeInterface<N> for Validator<N, C> {
    /// Shuts down the node.
    async fn shut_down(&self) {
        info!("Shutting down...");

        // Shut down the node.
        trace!("Shutting down the node...");
        self.shutdown
            .store(true, std::sync::atomic::Ordering::Release);

        // Abort the tasks.
        trace!("Shutting down the validator...");
        self.handles.lock().iter().for_each(|handle| handle.abort());

        // Shut down the router.
        self.router.shut_down().await;

        // Shut down consensus.
        trace!("Shutting down consensus...");
        self.consensus.shut_down().await;

        info!("Node has shut down.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snarkvm::prelude::{
        store::{helpers::memory::ConsensusMemory, ConsensusStore},
        MainnetV0, VM,
    };

    use anyhow::bail;
    use rand::SeedableRng;
    use rand_chacha::ChaChaRng;
    use std::str::FromStr;

    type CurrentNetwork = MainnetV0;

    /// Use `RUST_MIN_STACK=67108864 cargo test --release profiler --features timer` to run this test.
    #[ignore]
    #[tokio::test]
    async fn test_profiler() -> Result<()> {
        // Specify the node attributes.
        let node = SocketAddr::from_str("0.0.0.0:4130").unwrap();
        let rest = SocketAddr::from_str("0.0.0.0:3030").unwrap();
        let storage_mode = StorageMode::Development(0);
        let dev_txs = true;

        // Initialize an (insecure) fixed RNG.
        let mut rng = ChaChaRng::seed_from_u64(1234567890u64);
        // Initialize the account.
        let account = Account::<CurrentNetwork>::new(&mut rng).unwrap();
        // Initialize a new VM.
        let vm = VM::from(ConsensusStore::<
            CurrentNetwork,
            ConsensusMemory<CurrentNetwork>,
        >::open(None)?)?;
        // Initialize the genesis block.
        let genesis = vm.genesis_beacon(account.private_key(), &mut rng)?;

        println!("Initializing validator node...");

        let validator = Validator::<CurrentNetwork, ConsensusMemory<CurrentNetwork>>::new(
            node,
            None,
            Some(rest),
            10,
            account,
            genesis,
            storage_mode,
            dev_txs,
            Default::default(),
        )
        .await
        .unwrap();

        println!(
            "Loaded validator node with {} blocks",
            validator.ledger.latest_height(),
        );

        bail!("\n\nRemember to #[ignore] this test!\n\n")
    }
}
