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

use crate::helpers::{BFTSender, Storage};
use snarkos_lite_node_bft_ledger_service::LedgerService;
use snarkos_lite_node_sync::locators::{BlockLocators, CHECKPOINT_INTERVAL, NUM_RECENT_BLOCKS};
use snarkvm::{
    console::network::Network,
    ledger::authority::Authority,
    prelude::{cfg_into_iter, cfg_iter},
};

use anyhow::{bail, Result};
use indexmap::IndexMap;
use rayon::prelude::*;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex as TMutex, OnceCell};

use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone)]
pub struct Sync<N: Network> {
    /// The storage.
    storage: Storage<N>,
    /// The ledger service.
    ledger: Arc<dyn LedgerService<N>>,
    /// The BFT sender.
    bft_sender: Arc<OnceCell<BFTSender<N>>>,
    /// The sync lock.
    sync_lock: Arc<TMutex<()>>,
    /// The boolean indicator of whether the node is synced up to the latest block (within the given tolerance).
    is_block_synced: Arc<AtomicBool>,
}

impl<N: Network> Sync<N> {
    /// Initializes a new sync instance.
    pub fn new(storage: Storage<N>, ledger: Arc<dyn LedgerService<N>>) -> Self {
        // Return the sync instance.
        Self {
            storage,
            ledger,
            bft_sender: Default::default(),
            sync_lock: Default::default(),
            is_block_synced: Default::default(),
        }
    }

    /// Initializes the sync module and sync the storage with the ledger at bootup.
    pub async fn initialize(&self, bft_sender: Option<BFTSender<N>>) -> Result<()> {
        // If a BFT sender was provided, set it.
        if let Some(bft_sender) = bft_sender {
            self.bft_sender
                .set(bft_sender)
                .expect("BFT sender already set in gateway");
        }

        info!("Syncing storage with the ledger...");

        // Sync the storage with the ledger.
        self.sync_storage_with_ledger_at_bootup().await
    }

    /// Starts the sync module.
    pub async fn run(&self) -> Result<()> {
        info!("Starting the sync module...");

        // Update the sync status.
        self.is_block_synced.store(true, Ordering::SeqCst);

        // Update the `IS_SYNCED` metric.
        #[cfg(feature = "metrics")]
        metrics::gauge(metrics::bft::IS_SYNCED, true);

        Ok(())
    }
}

// Methods to manage storage.
impl<N: Network> Sync<N> {
    /// Syncs the storage with the ledger at bootup.
    async fn sync_storage_with_ledger_at_bootup(&self) -> Result<()> {
        // Retrieve the latest block in the ledger.
        let latest_block = self.ledger.latest_block();

        // Retrieve the block height.
        let block_height = latest_block.height();
        // Determine the number of maximum number of blocks that would have been garbage collected.
        let max_gc_blocks = u32::try_from(self.storage.max_gc_rounds())?.saturating_div(2);
        // Determine the earliest height, conservatively set to the block height minus the max GC rounds.
        // By virtue of the BFT protocol, we can guarantee that all GC range blocks will be loaded.
        let gc_height = block_height.saturating_sub(max_gc_blocks);
        // Retrieve the blocks.
        let blocks = self
            .ledger
            .get_blocks(gc_height..block_height.saturating_add(1))?;

        // Acquire the sync lock.
        let _lock = self.sync_lock.lock().await;

        debug!(
            "Syncing storage with the ledger from block {} to {}...",
            gc_height,
            block_height.saturating_add(1)
        );

        /* Sync storage */

        // Sync the height with the block.
        self.storage.sync_height_with_block(latest_block.height());
        // Sync the round with the block.
        self.storage.sync_round_with_block(latest_block.round());
        // Perform GC on the latest block round.
        self.storage
            .garbage_collect_certificates(latest_block.round());
        // Iterate over the blocks.
        for block in &blocks {
            // If the block authority is a subdag, then sync the batch certificates with the block.
            if let Authority::Quorum(subdag) = block.authority() {
                // Reconstruct the unconfirmed transactions.
                let unconfirmed_transactions = cfg_iter!(block.transactions())
                    .filter_map(|tx| {
                        tx.to_unconfirmed_transaction()
                            .map(|unconfirmed| (unconfirmed.id(), unconfirmed))
                            .ok()
                    })
                    .collect::<HashMap<_, _>>();

                // Iterate over the certificates.
                for certificates in subdag.values().cloned() {
                    cfg_into_iter!(certificates).for_each(|certificate| {
                        self.storage.sync_certificate_with_block(
                            block,
                            certificate,
                            &unconfirmed_transactions,
                        );
                    });
                }
            }
        }

        /* Sync the BFT DAG */

        // Construct a list of the certificates.
        let certificates = blocks
            .iter()
            .flat_map(|block| {
                match block.authority() {
                    // If the block authority is a beacon, then skip the block.
                    Authority::Beacon(_) => None,
                    // If the block authority is a subdag, then retrieve the certificates.
                    Authority::Quorum(subdag) => {
                        Some(subdag.values().flatten().cloned().collect::<Vec<_>>())
                    }
                }
            })
            .flatten()
            .collect::<Vec<_>>();

        // If a BFT sender was provided, send the certificates to the BFT.
        if let Some(bft_sender) = self.bft_sender.get() {
            // Await the callback to continue.
            if let Err(e) = bft_sender
                .tx_sync_bft_dag_at_bootup
                .send(certificates)
                .await
            {
                bail!("Failed to update the BFT DAG from sync: {e}");
            }
        }

        Ok(())
    }
}

// Methods to assist with the block sync module.
impl<N: Network> Sync<N> {
    /// Returns `true` if the node is synced and has connected peers.
    pub fn is_synced(&self) -> bool {
        self.is_block_synced.load(Ordering::SeqCst)
    }

    /// Returns the number of blocks the node is behind the greatest peer height.
    pub fn num_blocks_behind(&self) -> u32 {
        0u32
    }

    /// Returns `true` if the node is in gateway mode.
    pub const fn is_gateway_mode(&self) -> bool {
        true
    }

    /// Returns the current block locators of the node.
    pub fn get_block_locators(&self) -> Result<BlockLocators<N>> {
        // Retrieve the latest block height.
        let latest_height = self.ledger.latest_block_height();

        // Initialize the recents map.
        let mut recents = IndexMap::with_capacity(NUM_RECENT_BLOCKS);
        // Retrieve the recent block hashes.
        for height in latest_height.saturating_sub((NUM_RECENT_BLOCKS - 1) as u32)..=latest_height {
            recents.insert(height, self.ledger.get_block_hash(height)?);
        }

        // Initialize the checkpoints map.
        let mut checkpoints =
            IndexMap::with_capacity((latest_height / CHECKPOINT_INTERVAL + 1).try_into()?);
        // Retrieve the checkpoint block hashes.
        for height in (0..=latest_height).step_by(CHECKPOINT_INTERVAL as usize) {
            checkpoints.insert(height, self.ledger.get_block_hash(height)?);
        }

        // Construct the block locators.
        BlockLocators::new(recents, checkpoints)
    }
}

impl<N: Network> Sync<N> {
    /// Shuts down the primary.
    pub async fn shut_down(&self) {
        info!("Shutting down the sync module...");
        // Acquire the sync lock.
        let _lock = self.sync_lock.lock().await;
    }
}
