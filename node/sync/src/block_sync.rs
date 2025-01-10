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

use crate::locators::{BlockLocators, CHECKPOINT_INTERVAL, NUM_RECENT_BLOCKS};
use snarkos_lite_node_bft_ledger_service::LedgerService;
use snarkvm::prelude::Network;

use anyhow::Result;
use indexmap::IndexMap;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

#[cfg(not(test))]
pub const REDUNDANCY_FACTOR: usize = 1;
#[cfg(test)]
pub const REDUNDANCY_FACTOR: usize = 3;

/// The maximum number of blocks tolerated before the primary is considered behind its peers.
pub const MAX_BLOCKS_BEHIND: u32 = 1; // blocks

/// This is a dummy IP address that is used to represent the local node.
/// Note: This here does not need to be a real IP address, but it must be unique/distinct from all other connections.
pub const DUMMY_SELF_IP: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum BlockSyncMode {
    Router,
    Gateway,
}

impl BlockSyncMode {
    /// Returns `true` if the node is in router mode.
    pub const fn is_router(&self) -> bool {
        matches!(self, Self::Router)
    }

    /// Returns `true` if the node is in gateway mode.
    pub const fn is_gateway(&self) -> bool {
        matches!(self, Self::Gateway)
    }
}

/// A struct that keeps track of the current block sync state.
#[derive(Clone, Debug)]
pub struct BlockSync<N: Network> {
    /// The block sync mode.
    mode: BlockSyncMode,
    /// The canonical map of block height to block hash.
    /// This map is a linearly-increasing map of block heights to block hashes,
    /// updated solely from the ledger and candidate blocks (not from peers' block locators, to ensure there are no forks).
    canon: Arc<dyn LedgerService<N>>,
    /// The boolean indicator of whether the node is synced up to the latest block (within the given tolerance).
    is_block_synced: Arc<AtomicBool>,
}

impl<N: Network> BlockSync<N> {
    /// Initializes a new block sync module.
    pub fn new(mode: BlockSyncMode, ledger: Arc<dyn LedgerService<N>>) -> Self {
        Self {
            mode,
            canon: ledger,
            is_block_synced: Default::default(),
        }
    }

    /// Returns the block sync mode.
    #[inline]
    pub const fn mode(&self) -> BlockSyncMode {
        self.mode
    }

    /// Returns `true` if the node is synced up to the latest block (within the given tolerance).
    #[inline]
    pub fn is_block_synced(&self) -> bool {
        self.is_block_synced.load(Ordering::SeqCst)
    }
}

impl<N: Network> BlockSync<N> {
    /// Returns the block locators.
    #[inline]
    pub fn get_block_locators(&self) -> Result<BlockLocators<N>> {
        // Retrieve the latest block height.
        let latest_height = self.canon.latest_block_height();

        // Initialize the recents map.
        let mut recents = IndexMap::with_capacity(NUM_RECENT_BLOCKS);
        // Retrieve the recent block hashes.
        for height in latest_height.saturating_sub((NUM_RECENT_BLOCKS - 1) as u32)..=latest_height {
            recents.insert(height, self.canon.get_block_hash(height)?);
        }

        // Initialize the checkpoints map.
        let mut checkpoints =
            IndexMap::with_capacity((latest_height / CHECKPOINT_INTERVAL + 1).try_into()?);
        // Retrieve the checkpoint block hashes.
        for height in (0..=latest_height).step_by(CHECKPOINT_INTERVAL as usize) {
            checkpoints.insert(height, self.canon.get_block_hash(height)?);
        }

        // Construct the block locators.
        BlockLocators::new(recents, checkpoints)
    }

    /// Performs one iteration of the block sync.
    #[inline]
    pub async fn try_block_sync(&self) {
        // Update the sync status.
        self.is_block_synced.store(true, Ordering::SeqCst);

        // Update the `IS_SYNCED` metric.
        #[cfg(feature = "metrics")]
        metrics::gauge(metrics::bft::IS_SYNCED, true);
    }
}
