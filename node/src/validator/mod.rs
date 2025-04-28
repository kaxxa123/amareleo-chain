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

use amareleo_chain_account::Account;
use amareleo_chain_tracing::{TracingHandler, TracingHandlerGuard};
use amareleo_node_bft::{helpers::init_primary_channels, ledger_service::CoreLedgerService};
use amareleo_node_consensus::Consensus;
use amareleo_node_rest::Rest;

use snarkvm::prelude::{Ledger, Network, block::Block, store::ConsensusStorage};

use aleo_std::StorageMode;
use anyhow::Result;
use std::{
    net::SocketAddr,
    sync::{Arc, atomic::AtomicBool},
};
use tracing::subscriber::DefaultGuard;

/// A validator is a full node, capable of validating blocks.
#[derive(Clone)]
pub struct Validator<N: Network, C: ConsensusStorage<N>> {
    /// The ledger of the node.
    ledger: Ledger<N, C>,
    /// The consensus module of the node.
    consensus: Consensus<N>,
    /// The REST server of the node.
    rest: Option<Rest<N, C>>,
    /// Tracing handle
    tracing: Option<TracingHandler>,
    /// The shutdown signal.
    shutdown: Arc<AtomicBool>,
}

impl<N: Network, C: ConsensusStorage<N>> TracingHandlerGuard for Validator<N, C> {
    /// Retruns tracing guard
    fn get_tracing_guard(&self) -> Option<DefaultGuard> {
        self.tracing.as_ref().and_then(|trace_handle| trace_handle.get_tracing_guard())
    }
}

impl<N: Network, C: ConsensusStorage<N>> Validator<N, C> {
    /// Initializes a new validator node.
    pub async fn new(
        rest_ip: SocketAddr,
        rest_rps: u32,
        account: Account<N>,
        genesis: Block<N>,
        keep_state: bool,
        storage_mode: StorageMode,
        tracing: Option<TracingHandler>,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        // Initialize the ledger.
        let ledger = Ledger::load(genesis, storage_mode.clone())?;
        let tracing_: TracingHandler = tracing.clone().into();

        // Initialize and start the Consensus
        guard_info!(tracing_, "Starting Consensus...");
        let ledger_service = Arc::new(CoreLedgerService::new(ledger.clone(), tracing.clone(), shutdown.clone()));
        let mut consensus =
            Consensus::new(account.clone(), ledger_service.clone(), keep_state, storage_mode.clone(), tracing.clone())?;

        let (primary_sender, primary_receiver) = init_primary_channels::<N>();
        consensus.run(primary_sender, primary_receiver).await?;

        // Initialize the REST server.
        guard_info!(tracing_, "Starting the REST server...");
        let rest_srv = Rest::start(rest_ip, rest_rps, Some(consensus.clone()), ledger.clone(), tracing.clone()).await;
        if let Err(err) = rest_srv {
            guard_error!(tracing_, "Failed to start REST server: {:?}", err);
            consensus.shut_down().await;
            return Err(err);
        }

        // Return the node.
        Ok(Self { ledger, consensus, rest: Some(rest_srv.unwrap()), tracing, shutdown })
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
    /// Shuts down the node.
    pub async fn shut_down(&self) {
        // Flag that we are shutting down.
        // This will prevent adding further blocks to the ledger.
        guard_info!(self, "Shutting down...");
        self.shutdown.store(true, std::sync::atomic::Ordering::Release);

        // Shut down rest server.
        if let Some(rest) = &self.rest {
            guard_trace!(self, "Shutting down the REST server...");
            rest.shut_down().await;
        }

        // Shut down consensus.
        guard_trace!(self, "Shutting down consensus...");
        self.consensus.shut_down().await;

        guard_info!(self, "Node has shut down.");
        guard_info!(self, "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amareleo_node_bft::DEVELOPMENT_MODE_RNG_SEED;

    use snarkvm::prelude::{
        MainnetV0,
        VM,
        store::{ConsensusStore, helpers::memory::ConsensusMemory},
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
        let rest = SocketAddr::from_str("0.0.0.0:3030").unwrap();
        let storage_mode = StorageMode::Development(0);

        // Initialize an (insecure) fixed RNG.
        let mut rng = ChaChaRng::seed_from_u64(DEVELOPMENT_MODE_RNG_SEED);
        // Initialize the account.
        let account = Account::<CurrentNetwork>::new(&mut rng).unwrap();
        // Initialize a new VM.
        let vm = VM::from(ConsensusStore::<CurrentNetwork, ConsensusMemory<CurrentNetwork>>::open(
            StorageMode::new_test(None),
        )?)?;
        // Initialize the genesis block.
        let genesis = vm.genesis_beacon(account.private_key(), &mut rng)?;

        println!("Initializing validator node...");

        let validator = Validator::<CurrentNetwork, ConsensusMemory<CurrentNetwork>>::new(
            rest,
            10,
            account,
            genesis,
            false,
            storage_mode,
            None,
            Default::default(),
        )
        .await
        .unwrap();

        println!("Loaded validator node with {} blocks", validator.ledger.latest_height(),);

        bail!("\n\nRemember to #[ignore] this test!\n\n")
    }
}
