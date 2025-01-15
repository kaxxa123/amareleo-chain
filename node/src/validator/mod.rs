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

use crate::traits::NodeInterface;
use snarkos_lite_account::Account;
use snarkos_lite_node_bft::{helpers::init_primary_channels, ledger_service::CoreLedgerService};
use snarkos_lite_node_consensus::Consensus;
use snarkos_lite_node_rest::Rest;
use snarkos_lite_node_router::{
    messages::{NodeType, PuzzleResponse, UnconfirmedSolution},
    Heartbeat, Inbound, Outbound, Router, Routing,
};
use snarkos_lite_node_sync::{BlockSync, BlockSyncMode};
use snarkos_lite_node_tcp::{
    protocols::{Disconnect, Handshake, OnConnect, Reading, Writing},
    Config, Tcp, P2P,
};
use snarkvm::prelude::{block::Block, store::ConsensusStorage, Ledger, Network};

use aleo_std::StorageMode;
use anyhow::Result;
use core::future::Future;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
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
    /// The REST server of the node.
    rest: Option<Rest<N, C>>,
    /// The spawned handles.
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
    /// The shutdown signal.
    shutdown: Arc<AtomicBool>,
}

impl<N: Network, C: ConsensusStorage<N>> Validator<N, C> {
    /// Initializes a new validator node.
    pub async fn new(
        node_ip: SocketAddr,
        rest_ip: Option<SocketAddr>,
        rest_rps: u32,
        account: Account<N>,
        genesis: Block<N>,
        storage_mode: StorageMode,
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
            &trusted_validators,
            storage_mode.clone(),
        )?;
        // Initialize the primary channels.
        let (primary_sender, primary_receiver) = init_primary_channels::<N>();
        // Start the consensus.
        consensus.run(primary_sender, primary_receiver).await?;

        // Initialize the node.
        let mut node = Self {
            ledger: ledger.clone(),
            consensus: consensus.clone(),
            rest: None,
            handles: Default::default(),
            shutdown,
        };

        // Initialize the REST server.
        if let Some(rest_ip) = rest_ip {
            node.rest =
                Some(Rest::start(rest_ip, rest_rps, Some(consensus), ledger.clone()).await?);
        }

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
    pub fn rest(&self) -> &Option<Rest<N, C>> {
        &self.rest
    }
}

impl<N: Network, C: ConsensusStorage<N>> Validator<N, C> {
    /// Handles OS signals for the node to intercept and perform a clean shutdown.
    /// The optional `shutdown_flag` flag can be used to cleanly terminate the syncing process.
    fn handle_signals(shutdown_flag: Arc<AtomicBool>) -> Arc<OnceCell<Self>> {
        // In order for the signal handler to be started as early as possible, a reference to the node needs
        // to be passed to it at a later time.
        let node: Arc<OnceCell<Self>> = Default::default();

        #[cfg(target_family = "unix")]
        fn signal_listener() -> impl Future<Output = io::Result<()>> {
            use tokio::signal::unix::{signal, SignalKind};

            // Handle SIGINT, SIGTERM, SIGQUIT, and SIGHUP.
            let mut s_int = signal(SignalKind::interrupt()).unwrap();
            let mut s_term = signal(SignalKind::terminate()).unwrap();
            let mut s_quit = signal(SignalKind::quit()).unwrap();
            let mut s_hup = signal(SignalKind::hangup()).unwrap();

            // Return when any of the signals above is received.
            async move {
                tokio::select!(
                    _ = s_int.recv() => (),
                    _ = s_term.recv() => (),
                    _ = s_quit.recv() => (),
                    _ = s_hup.recv() => (),
                );
                Ok(())
            }
        }
        #[cfg(not(target_family = "unix"))]
        fn signal_listener() -> impl Future<Output = io::Result<()>> {
            tokio::signal::ctrl_c()
        }

        let node_clone = node.clone();
        tokio::task::spawn(async move {
            match signal_listener().await {
                Ok(()) => {
                    warn!("==========================================================================================");
                    warn!("⚠️  Attention - Starting the graceful shutdown procedure (ETA: 30 seconds)...");
                    warn!("⚠️  Attention - To avoid DATA CORRUPTION, do NOT interrupt amareleo (or press Ctrl+C again)");
                    warn!("⚠️  Attention - Please wait until the shutdown gracefully completes (ETA: 30 seconds)");
                    warn!("==========================================================================================");

                    match node_clone.get() {
                        // If the node is already initialized, then shut it down.
                        Some(node) => node.shut_down().await,
                        // Otherwise, if the node is not yet initialized, then set the shutdown flag directly.
                        None => shutdown_flag.store(true, Ordering::Relaxed),
                    }

                    // A best-effort attempt to let any ongoing activity conclude.
                    tokio::time::sleep(Duration::from_secs(3)).await;

                    // Terminate the process.
                    std::process::exit(0);
                }
                Err(error) => error!("tokio::signal::ctrl_c encountered an error: {}", error),
            }
        });

        node
    }

    /// Spawns a task with the given future; it should only be used for long-running tasks.
    pub fn spawn<T: Future<Output = ()> + Send + 'static>(&self, future: T) {
        self.handles.lock().push(tokio::spawn(future));
    }
}

impl<N: Network, C: ConsensusStorage<N>> Validator<N, C> {
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
            Some(rest),
            10,
            account,
            genesis,
            storage_mode,
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
