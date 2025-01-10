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

use crate::{
    helpers::{BFTSender, Storage},
    PRIMARY_PING_IN_MS,
};
use snarkos_lite_node_bft_ledger_service::LedgerService;
use snarkos_lite_node_sync::locators::{BlockLocators, CHECKPOINT_INTERVAL, NUM_RECENT_BLOCKS};
use snarkvm::{
    console::network::Network,
    ledger::{authority::Authority, block::Block, narwhal::BatchCertificate},
    prelude::{cfg_into_iter, cfg_iter},
};

use anyhow::{bail, Result};
use indexmap::IndexMap;
use parking_lot::Mutex;
use rayon::prelude::*;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    sync::{Mutex as TMutex, OnceCell},
    task::JoinHandle,
};

use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone)]
pub struct Sync<N: Network> {
    /// The storage.
    storage: Storage<N>,
    /// The ledger service.
    ledger: Arc<dyn LedgerService<N>>,
    /// The BFT sender.
    bft_sender: Arc<OnceCell<BFTSender<N>>>,
    /// The spawned handles.
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
    /// The response lock.
    response_lock: Arc<TMutex<()>>,
    /// The sync lock.
    sync_lock: Arc<TMutex<()>>,
    /// The latest block responses.
    latest_block_responses: Arc<TMutex<HashMap<u32, Block<N>>>>,
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
            handles: Default::default(),
            response_lock: Default::default(),
            sync_lock: Default::default(),
            latest_block_responses: Default::default(),
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

        // Start the block sync loop.
        let self_ = self.clone();
        self.handles.lock().push(tokio::spawn(async move {
            // Sleep briefly to allow an initial primary ping to come in prior to entering the loop.
            // Ideally, a node does not consider itself synced when it has not received
            // any block locators from peer. However, in the initial bootup of validators,
            // this needs to happen, so we use this additional sleep as a grace period.
            tokio::time::sleep(Duration::from_millis(PRIMARY_PING_IN_MS)).await;
            loop {
                // Sleep briefly to avoid triggering spam detection.
                tokio::time::sleep(Duration::from_millis(PRIMARY_PING_IN_MS)).await;

                // Update the sync status.
                self_.is_block_synced.store(true, Ordering::SeqCst);

                // Update the `IS_SYNCED` metric.
                #[cfg(feature = "metrics")]
                metrics::gauge(metrics::bft::IS_SYNCED, true);

                // Sync the storage with the blocks.
                if let Err(e) = self_.sync_storage_with_blocks().await {
                    error!("Unable to sync storage with blocks - {e}");
                }

                // If the node is synced, clear the `latest_block_responses`.
                if self_.is_synced() {
                    self_.latest_block_responses.lock().await.clear();
                }
            }
        }));

        Ok(())
    }
}

// Methods to manage storage.
impl<N: Network> Sync<N> {
    /// Syncs the storage with the ledger at bootup.
    pub async fn sync_storage_with_ledger_at_bootup(&self) -> Result<()> {
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

    /// Syncs the storage with the given blocks.
    pub async fn sync_storage_with_blocks(&self) -> Result<()> {
        // Acquire the response lock.
        let _lock = self.response_lock.lock().await;

        // Retrieve the latest block height.
        let current_height = self.ledger.latest_block_height() + 1;

        // Retrieve the maximum block height of the peers.
        let tip = 0u32;
        // Determine the number of maximum number of blocks that would have been garbage collected.
        let max_gc_blocks = u32::try_from(self.storage.max_gc_rounds())?.saturating_div(2);
        // Determine the maximum height that the peer would have garbage collected.
        let max_gc_height = tip.saturating_sub(max_gc_blocks);

        // Determine if we can sync the ledger without updating the BFT first.
        if current_height <= max_gc_height {
            // Sync the storage with the ledger if we should transition to the BFT sync.
            if current_height > max_gc_height {
                if let Err(e) = self.sync_storage_with_ledger_at_bootup().await {
                    error!("BFT sync (with bootup routine) failed - {e}");
                }
            }
        }

        Ok(())
    }

    /// Syncs the storage with the given blocks.
    pub async fn sync_storage_with_block(&self, block: Block<N>) -> Result<()> {
        // Acquire the sync lock.
        let _lock = self.sync_lock.lock().await;
        // Acquire the latest block responses lock.
        let mut latest_block_responses = self.latest_block_responses.lock().await;

        // If this block has already been processed, return early.
        if self.ledger.contains_block_height(block.height())
            || latest_block_responses.contains_key(&block.height())
        {
            return Ok(());
        }

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
                cfg_into_iter!(certificates.clone()).for_each(|certificate| {
                    // Sync the batch certificate with the block.
                    self.storage.sync_certificate_with_block(
                        &block,
                        certificate.clone(),
                        &unconfirmed_transactions,
                    );
                });

                // Sync the BFT DAG with the certificates.
                for certificate in certificates {
                    // If a BFT sender was provided, send the certificate to the BFT.
                    if let Some(bft_sender) = self.bft_sender.get() {
                        // Await the callback to continue.
                        if let Err(e) = bft_sender.send_sync_bft(certificate).await {
                            bail!("Sync - {e}");
                        };
                    }
                }
            }
        }

        // Fetch the latest block height.
        let latest_block_height = self.ledger.latest_block_height();

        // Insert the latest block response.
        latest_block_responses.insert(block.height(), block);
        // Clear the latest block responses of older blocks.
        latest_block_responses.retain(|height, _| *height > latest_block_height);

        // Get a list of contiguous blocks from the latest block responses.
        let contiguous_blocks: Vec<Block<N>> = (latest_block_height.saturating_add(1)..)
            .take_while(|&k| latest_block_responses.contains_key(&k))
            .filter_map(|k| latest_block_responses.get(&k).cloned())
            .collect();

        // Check if the block response is ready to be added to the ledger.
        // Ensure that the previous block's leader certificate meets the availability threshold
        // based on the certificates in the current block.
        // If the availability threshold is not met, process the next block and check if it is linked to the current block.
        // Note: We do not advance to the most recent block response because we would be unable to
        // validate if the leader certificate in the block has been certified properly.
        for next_block in contiguous_blocks.into_iter() {
            // Retrieve the height of the next block.
            let next_block_height = next_block.height();

            // Fetch the leader certificate and the relevant rounds.
            let leader_certificate = match next_block.authority() {
                Authority::Quorum(subdag) => subdag.leader_certificate().clone(),
                _ => bail!("Received a block with an unexpected authority type."),
            };
            let commit_round = leader_certificate.round();
            let certificate_round = commit_round.saturating_add(1);

            // Get the committee lookback for the commit round.
            let committee_lookback = self.ledger.get_committee_lookback_for_round(commit_round)?;
            // Retrieve all of the certificates for the **certificate** round.
            let certificates = self.storage.get_certificates_for_round(certificate_round);
            // Construct a set over the authors who included the leader's certificate in the certificate round.
            let authors = certificates
                .iter()
                .filter_map(|c| {
                    match c
                        .previous_certificate_ids()
                        .contains(&leader_certificate.id())
                    {
                        true => Some(c.author()),
                        false => None,
                    }
                })
                .collect();

            debug!("Validating sync block {next_block_height} at round {commit_round}...");
            // Check if the leader is ready to be committed.
            if committee_lookback.is_availability_threshold_reached(&authors) {
                // Initialize the current certificate.
                let mut current_certificate = leader_certificate;
                // Check if there are any linked blocks that need to be added.
                let mut blocks_to_add = vec![next_block];

                // Check if there are other blocks to process based on `is_linked`.
                for height in
                    (self.ledger.latest_block_height().saturating_add(1)..next_block_height).rev()
                {
                    // Retrieve the previous block.
                    let Some(previous_block) = latest_block_responses.get(&height) else {
                        bail!("Block {height} is missing from the latest block responses.");
                    };
                    // Retrieve the previous certificate.
                    let previous_certificate = match previous_block.authority() {
                        Authority::Quorum(subdag) => subdag.leader_certificate().clone(),
                        _ => bail!("Received a block with an unexpected authority type."),
                    };
                    // Determine if there is a path between the previous certificate and the current certificate.
                    if self.is_linked(previous_certificate.clone(), current_certificate.clone())? {
                        debug!("Previous sync block {height} is linked to the current block {next_block_height}");
                        // Add the previous leader certificate to the list of certificates to commit.
                        blocks_to_add.insert(0, previous_block.clone());
                        // Update the current certificate to the previous leader certificate.
                        current_certificate = previous_certificate;
                    }
                }

                // Add the blocks to the ledger.
                for block in blocks_to_add {
                    // Check that the blocks are sequential and can be added to the ledger.
                    let block_height = block.height();
                    if block_height != self.ledger.latest_block_height().saturating_add(1) {
                        warn!("Skipping block {block_height} from the latest block responses - not sequential.");
                        continue;
                    }

                    let self_ = self.clone();
                    tokio::task::spawn_blocking(move || {
                        // Check the next block.
                        self_.ledger.check_next_block(&block)?;
                        // Attempt to advance to the next block.
                        self_.ledger.advance_to_next_block(&block)?;

                        // Sync the height with the block.
                        self_.storage.sync_height_with_block(block.height());
                        // Sync the round with the block.
                        self_.storage.sync_round_with_block(block.round());

                        Ok::<(), anyhow::Error>(())
                    })
                    .await??;
                    // Remove the block height from the latest block responses.
                    latest_block_responses.remove(&block_height);
                }
            } else {
                debug!(
                    "Availability threshold was not reached for block {next_block_height} at round {commit_round}. Checking next block..."
                );
            }
        }

        Ok(())
    }

    /// Returns `true` if there is a path from the previous certificate to the current certificate.
    fn is_linked(
        &self,
        previous_certificate: BatchCertificate<N>,
        current_certificate: BatchCertificate<N>,
    ) -> Result<bool> {
        // Initialize the list containing the traversal.
        let mut traversal = vec![current_certificate.clone()];
        // Iterate over the rounds from the current certificate to the previous certificate.
        for round in (previous_certificate.round()..current_certificate.round()).rev() {
            // Retrieve all of the certificates for this past round.
            let certificates = self.storage.get_certificates_for_round(round);
            // Filter the certificates to only include those that are in the traversal.
            traversal = certificates
                .into_iter()
                .filter(|p| {
                    traversal
                        .iter()
                        .any(|c| c.previous_certificate_ids().contains(&p.id()))
                })
                .collect();
        }
        Ok(traversal.contains(&previous_certificate))
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
        // Acquire the response lock.
        let _lock = self.response_lock.lock().await;
        // Acquire the sync lock.
        let _lock = self.sync_lock.lock().await;
        // Abort the tasks.
        self.handles.lock().iter().for_each(|handle| handle.abort());
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    use crate::{
        helpers::now, ledger_service::CoreLedgerService, storage_service::BFTMemoryService,
    };
    use snarkos_lite_account::Account;
    use snarkvm::{
        console::{
            account::{Address, PrivateKey},
            network::MainnetV0,
        },
        ledger::{
            narwhal::{BatchCertificate, BatchHeader, Subdag},
            store::{helpers::memory::ConsensusMemory, ConsensusStore},
        },
        prelude::{Ledger, VM},
        utilities::TestRng,
    };

    use aleo_std::StorageMode;
    use indexmap::IndexSet;
    use rand::Rng;
    use std::collections::BTreeMap;

    type CurrentNetwork = MainnetV0;
    type CurrentLedger = Ledger<CurrentNetwork, ConsensusMemory<CurrentNetwork>>;
    type CurrentConsensusStore = ConsensusStore<CurrentNetwork, ConsensusMemory<CurrentNetwork>>;

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn test_commit_via_is_linked() -> anyhow::Result<()> {
        let rng = &mut TestRng::default();
        // Initialize the round parameters.
        let max_gc_rounds = BatchHeader::<CurrentNetwork>::MAX_GC_ROUNDS as u64;
        let commit_round = 2;

        // Initialize the store.
        let store = CurrentConsensusStore::open(None).unwrap();
        let account: Account<CurrentNetwork> = Account::new(rng)?;

        // Create a genesis block with a seeded RNG to reproduce the same genesis private keys.
        let seed: u64 = rng.gen();
        let genesis_rng = &mut TestRng::from_seed(seed);
        let genesis = VM::from(store)
            .unwrap()
            .genesis_beacon(account.private_key(), genesis_rng)
            .unwrap();

        // Extract the private keys from the genesis committee by using the same RNG to sample private keys.
        let genesis_rng = &mut TestRng::from_seed(seed);
        let private_keys = [
            *account.private_key(),
            PrivateKey::new(genesis_rng)?,
            PrivateKey::new(genesis_rng)?,
            PrivateKey::new(genesis_rng)?,
        ];

        // Initialize the ledger with the genesis block.
        let ledger = CurrentLedger::load(genesis.clone(), StorageMode::Production).unwrap();
        // Initialize the ledger.
        let core_ledger = Arc::new(CoreLedgerService::new(ledger.clone(), Default::default()));

        // Sample 5 rounds of batch certificates starting at the genesis round from a static set of 4 authors.
        let (round_to_certificates_map, committee) = {
            let addresses = vec![
                Address::try_from(private_keys[0])?,
                Address::try_from(private_keys[1])?,
                Address::try_from(private_keys[2])?,
                Address::try_from(private_keys[3])?,
            ];

            let committee = ledger.latest_committee().unwrap();

            // Initialize a mapping from the round number to the set of batch certificates in the round.
            let mut round_to_certificates_map: HashMap<
                u64,
                IndexSet<BatchCertificate<CurrentNetwork>>,
            > = HashMap::new();
            let mut previous_certificates: IndexSet<BatchCertificate<CurrentNetwork>> =
                IndexSet::with_capacity(4);

            for round in 0..=commit_round + 8 {
                let mut current_certificates = IndexSet::new();
                let previous_certificate_ids: IndexSet<_> = if round == 0 || round == 1 {
                    IndexSet::new()
                } else {
                    previous_certificates.iter().map(|c| c.id()).collect()
                };
                let committee_id = committee.id();

                // Create a certificate for the leader.
                if round <= 5 {
                    let leader = committee.get_leader(round).unwrap();
                    let leader_index = addresses
                        .iter()
                        .position(|&address| address == leader)
                        .unwrap();
                    let non_leader_index = addresses
                        .iter()
                        .position(|&address| address != leader)
                        .unwrap();
                    for i in [leader_index, non_leader_index].into_iter() {
                        let batch_header = BatchHeader::new(
                            &private_keys[i],
                            round,
                            now(),
                            committee_id,
                            Default::default(),
                            previous_certificate_ids.clone(),
                            rng,
                        )
                        .unwrap();
                        // Sign the batch header.
                        let mut signatures = IndexSet::with_capacity(4);
                        for (j, private_key_2) in private_keys.iter().enumerate() {
                            if i != j {
                                signatures.insert(
                                    private_key_2.sign(&[batch_header.batch_id()], rng).unwrap(),
                                );
                            }
                        }
                        current_certificates
                            .insert(BatchCertificate::from(batch_header, signatures).unwrap());
                    }
                }

                // Create a certificate for each validator.
                if round > 5 {
                    for (i, private_key_1) in private_keys.iter().enumerate() {
                        let batch_header = BatchHeader::new(
                            private_key_1,
                            round,
                            now(),
                            committee_id,
                            Default::default(),
                            previous_certificate_ids.clone(),
                            rng,
                        )
                        .unwrap();
                        // Sign the batch header.
                        let mut signatures = IndexSet::with_capacity(4);
                        for (j, private_key_2) in private_keys.iter().enumerate() {
                            if i != j {
                                signatures.insert(
                                    private_key_2.sign(&[batch_header.batch_id()], rng).unwrap(),
                                );
                            }
                        }
                        current_certificates
                            .insert(BatchCertificate::from(batch_header, signatures).unwrap());
                    }
                }
                // Update the map of certificates.
                round_to_certificates_map.insert(round, current_certificates.clone());
                previous_certificates = current_certificates.clone();
            }
            (round_to_certificates_map, committee)
        };

        // Initialize the storage.
        let storage = Storage::new(
            core_ledger.clone(),
            Arc::new(BFTMemoryService::new()),
            max_gc_rounds,
        );
        // Insert certificates into storage.
        let mut certificates: Vec<BatchCertificate<CurrentNetwork>> = Vec::new();
        for i in 1..=commit_round + 8 {
            let c = (*round_to_certificates_map.get(&i).unwrap()).clone();
            certificates.extend(c);
        }
        for certificate in certificates.clone().iter() {
            storage.testing_only_insert_certificate_testing_only(certificate.clone());
        }

        // Create block 1.
        let leader_round_1 = commit_round;
        let leader_1 = committee.get_leader(leader_round_1).unwrap();
        let leader_certificate = storage
            .get_certificate_for_round_with_author(commit_round, leader_1)
            .unwrap();
        let block_1 = {
            let mut subdag_map: BTreeMap<u64, IndexSet<BatchCertificate<CurrentNetwork>>> =
                BTreeMap::new();
            let mut leader_cert_map = IndexSet::new();
            leader_cert_map.insert(leader_certificate.clone());
            let mut previous_cert_map = IndexSet::new();
            for cert in storage.get_certificates_for_round(commit_round - 1) {
                previous_cert_map.insert(cert);
            }
            subdag_map.insert(commit_round, leader_cert_map.clone());
            subdag_map.insert(commit_round - 1, previous_cert_map.clone());
            let subdag = Subdag::from(subdag_map.clone())?;
            core_ledger.prepare_advance_to_next_quorum_block(subdag, Default::default())?
        };
        // Insert block 1.
        core_ledger.advance_to_next_block(&block_1)?;

        // Create block 2.
        let leader_round_2 = commit_round + 2;
        let leader_2 = committee.get_leader(leader_round_2).unwrap();
        let leader_certificate_2 = storage
            .get_certificate_for_round_with_author(leader_round_2, leader_2)
            .unwrap();
        let block_2 = {
            let mut subdag_map_2: BTreeMap<u64, IndexSet<BatchCertificate<CurrentNetwork>>> =
                BTreeMap::new();
            let mut leader_cert_map_2 = IndexSet::new();
            leader_cert_map_2.insert(leader_certificate_2.clone());
            let mut previous_cert_map_2 = IndexSet::new();
            for cert in storage.get_certificates_for_round(leader_round_2 - 1) {
                previous_cert_map_2.insert(cert);
            }
            let mut prev_commit_cert_map_2 = IndexSet::new();
            for cert in storage.get_certificates_for_round(leader_round_2 - 2) {
                if cert != leader_certificate {
                    prev_commit_cert_map_2.insert(cert);
                }
            }
            subdag_map_2.insert(leader_round_2, leader_cert_map_2.clone());
            subdag_map_2.insert(leader_round_2 - 1, previous_cert_map_2.clone());
            subdag_map_2.insert(leader_round_2 - 2, prev_commit_cert_map_2.clone());
            let subdag_2 = Subdag::from(subdag_map_2.clone())?;
            core_ledger.prepare_advance_to_next_quorum_block(subdag_2, Default::default())?
        };
        // Insert block 2.
        core_ledger.advance_to_next_block(&block_2)?;

        // Create block 3
        let leader_round_3 = commit_round + 4;
        let leader_3 = committee.get_leader(leader_round_3).unwrap();
        let leader_certificate_3 = storage
            .get_certificate_for_round_with_author(leader_round_3, leader_3)
            .unwrap();
        let block_3 = {
            let mut subdag_map_3: BTreeMap<u64, IndexSet<BatchCertificate<CurrentNetwork>>> =
                BTreeMap::new();
            let mut leader_cert_map_3 = IndexSet::new();
            leader_cert_map_3.insert(leader_certificate_3.clone());
            let mut previous_cert_map_3 = IndexSet::new();
            for cert in storage.get_certificates_for_round(leader_round_3 - 1) {
                previous_cert_map_3.insert(cert);
            }
            let mut prev_commit_cert_map_3 = IndexSet::new();
            for cert in storage.get_certificates_for_round(leader_round_3 - 2) {
                if cert != leader_certificate_2 {
                    prev_commit_cert_map_3.insert(cert);
                }
            }
            subdag_map_3.insert(leader_round_3, leader_cert_map_3.clone());
            subdag_map_3.insert(leader_round_3 - 1, previous_cert_map_3.clone());
            subdag_map_3.insert(leader_round_3 - 2, prev_commit_cert_map_3.clone());
            let subdag_3 = Subdag::from(subdag_map_3.clone())?;
            core_ledger.prepare_advance_to_next_quorum_block(subdag_3, Default::default())?
        };
        // Insert block 3.
        core_ledger.advance_to_next_block(&block_3)?;

        // Initialize the syncing ledger.
        let syncing_ledger = Arc::new(CoreLedgerService::new(
            CurrentLedger::load(genesis, StorageMode::Production).unwrap(),
            Default::default(),
        ));
        // Initialize the sync module.
        let sync = Sync::new(storage.clone(), syncing_ledger.clone());
        // Try to sync block 1.
        sync.sync_storage_with_block(block_1).await?;
        assert_eq!(syncing_ledger.latest_block_height(), 1);
        // Try to sync block 2.
        sync.sync_storage_with_block(block_2).await?;
        assert_eq!(syncing_ledger.latest_block_height(), 2);
        // Try to sync block 3.
        sync.sync_storage_with_block(block_3).await?;
        assert_eq!(syncing_ledger.latest_block_height(), 3);
        // Ensure blocks 1 and 2 were added to the ledger.
        assert!(syncing_ledger.contains_block_height(1));
        assert!(syncing_ledger.contains_block_height(2));

        Ok(())
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn test_pending_certificates() -> anyhow::Result<()> {
        let rng = &mut TestRng::default();
        // Initialize the round parameters.
        let max_gc_rounds = BatchHeader::<CurrentNetwork>::MAX_GC_ROUNDS as u64;
        let commit_round = 2;

        // Initialize the store.
        let store = CurrentConsensusStore::open(None).unwrap();
        let account: Account<CurrentNetwork> = Account::new(rng)?;

        // Create a genesis block with a seeded RNG to reproduce the same genesis private keys.
        let seed: u64 = rng.gen();
        let genesis_rng = &mut TestRng::from_seed(seed);
        let genesis = VM::from(store)
            .unwrap()
            .genesis_beacon(account.private_key(), genesis_rng)
            .unwrap();

        // Extract the private keys from the genesis committee by using the same RNG to sample private keys.
        let genesis_rng = &mut TestRng::from_seed(seed);
        let private_keys = [
            *account.private_key(),
            PrivateKey::new(genesis_rng)?,
            PrivateKey::new(genesis_rng)?,
            PrivateKey::new(genesis_rng)?,
        ];
        // Initialize the ledger with the genesis block.
        let ledger = CurrentLedger::load(genesis.clone(), StorageMode::Production).unwrap();
        // Initialize the ledger.
        let core_ledger = Arc::new(CoreLedgerService::new(ledger.clone(), Default::default()));
        // Sample rounds of batch certificates starting at the genesis round from a static set of 4 authors.
        let (round_to_certificates_map, committee) = {
            // Initialize the committee.
            let committee = ledger.latest_committee().unwrap();
            // Initialize a mapping from the round number to the set of batch certificates in the round.
            let mut round_to_certificates_map: HashMap<
                u64,
                IndexSet<BatchCertificate<CurrentNetwork>>,
            > = HashMap::new();
            let mut previous_certificates: IndexSet<BatchCertificate<CurrentNetwork>> =
                IndexSet::with_capacity(4);

            for round in 0..=commit_round + 8 {
                let mut current_certificates = IndexSet::new();
                let previous_certificate_ids: IndexSet<_> = if round == 0 || round == 1 {
                    IndexSet::new()
                } else {
                    previous_certificates.iter().map(|c| c.id()).collect()
                };
                let committee_id = committee.id();
                // Create a certificate for each validator.
                for (i, private_key_1) in private_keys.iter().enumerate() {
                    let batch_header = BatchHeader::new(
                        private_key_1,
                        round,
                        now(),
                        committee_id,
                        Default::default(),
                        previous_certificate_ids.clone(),
                        rng,
                    )
                    .unwrap();
                    // Sign the batch header.
                    let mut signatures = IndexSet::with_capacity(4);
                    for (j, private_key_2) in private_keys.iter().enumerate() {
                        if i != j {
                            signatures.insert(
                                private_key_2.sign(&[batch_header.batch_id()], rng).unwrap(),
                            );
                        }
                    }
                    current_certificates
                        .insert(BatchCertificate::from(batch_header, signatures).unwrap());
                }

                // Update the map of certificates.
                round_to_certificates_map.insert(round, current_certificates.clone());
                previous_certificates = current_certificates.clone();
            }
            (round_to_certificates_map, committee)
        };

        // Initialize the storage.
        let storage = Storage::new(
            core_ledger.clone(),
            Arc::new(BFTMemoryService::new()),
            max_gc_rounds,
        );
        // Insert certificates into storage.
        let mut certificates: Vec<BatchCertificate<CurrentNetwork>> = Vec::new();
        for i in 1..=commit_round + 8 {
            let c = (*round_to_certificates_map.get(&i).unwrap()).clone();
            certificates.extend(c);
        }
        for certificate in certificates.clone().iter() {
            storage.testing_only_insert_certificate_testing_only(certificate.clone());
        }
        // Create block 1.
        let leader_round_1 = commit_round;
        let leader_1 = committee.get_leader(leader_round_1).unwrap();
        let leader_certificate = storage
            .get_certificate_for_round_with_author(commit_round, leader_1)
            .unwrap();
        let mut subdag_map: BTreeMap<u64, IndexSet<BatchCertificate<CurrentNetwork>>> =
            BTreeMap::new();
        let block_1 = {
            let mut leader_cert_map = IndexSet::new();
            leader_cert_map.insert(leader_certificate.clone());
            let mut previous_cert_map = IndexSet::new();
            for cert in storage.get_certificates_for_round(commit_round - 1) {
                previous_cert_map.insert(cert);
            }
            subdag_map.insert(commit_round, leader_cert_map.clone());
            subdag_map.insert(commit_round - 1, previous_cert_map.clone());
            let subdag = Subdag::from(subdag_map.clone())?;
            core_ledger.prepare_advance_to_next_quorum_block(subdag, Default::default())?
        };
        // Insert block 1.
        core_ledger.advance_to_next_block(&block_1)?;

        // Create block 2.
        let leader_round_2 = commit_round + 2;
        let leader_2 = committee.get_leader(leader_round_2).unwrap();
        let leader_certificate_2 = storage
            .get_certificate_for_round_with_author(leader_round_2, leader_2)
            .unwrap();
        let mut subdag_map_2: BTreeMap<u64, IndexSet<BatchCertificate<CurrentNetwork>>> =
            BTreeMap::new();
        let block_2 = {
            let mut leader_cert_map_2 = IndexSet::new();
            leader_cert_map_2.insert(leader_certificate_2.clone());
            let mut previous_cert_map_2 = IndexSet::new();
            for cert in storage.get_certificates_for_round(leader_round_2 - 1) {
                previous_cert_map_2.insert(cert);
            }
            subdag_map_2.insert(leader_round_2, leader_cert_map_2.clone());
            subdag_map_2.insert(leader_round_2 - 1, previous_cert_map_2.clone());
            let subdag_2 = Subdag::from(subdag_map_2.clone())?;
            core_ledger.prepare_advance_to_next_quorum_block(subdag_2, Default::default())?
        };
        // Insert block 2.
        core_ledger.advance_to_next_block(&block_2)?;

        // Create block 3
        let leader_round_3 = commit_round + 4;
        let leader_3 = committee.get_leader(leader_round_3).unwrap();
        let leader_certificate_3 = storage
            .get_certificate_for_round_with_author(leader_round_3, leader_3)
            .unwrap();
        let mut subdag_map_3: BTreeMap<u64, IndexSet<BatchCertificate<CurrentNetwork>>> =
            BTreeMap::new();
        let block_3 = {
            let mut leader_cert_map_3 = IndexSet::new();
            leader_cert_map_3.insert(leader_certificate_3.clone());
            let mut previous_cert_map_3 = IndexSet::new();
            for cert in storage.get_certificates_for_round(leader_round_3 - 1) {
                previous_cert_map_3.insert(cert);
            }
            subdag_map_3.insert(leader_round_3, leader_cert_map_3.clone());
            subdag_map_3.insert(leader_round_3 - 1, previous_cert_map_3.clone());
            let subdag_3 = Subdag::from(subdag_map_3.clone())?;
            core_ledger.prepare_advance_to_next_quorum_block(subdag_3, Default::default())?
        };
        // Insert block 3.
        core_ledger.advance_to_next_block(&block_3)?;

        /*
            Check that the pending certificates are computed correctly.
        */

        // Retrieve the pending certificates.
        let pending_certificates = storage.get_pending_certificates();
        // Check that all of the pending certificates are not contained in the ledger.
        for certificate in pending_certificates.clone() {
            assert!(!core_ledger
                .contains_certificate(&certificate.id())
                .unwrap_or(false));
        }
        // Initialize an empty set to be populated with the committed certificates in the block subdags.
        let mut committed_certificates: IndexSet<BatchCertificate<CurrentNetwork>> =
            IndexSet::new();
        {
            let subdag_maps = [&subdag_map, &subdag_map_2, &subdag_map_3];
            for subdag in subdag_maps.iter() {
                for subdag_certificates in subdag.values() {
                    committed_certificates.extend(subdag_certificates.iter().cloned());
                }
            }
        };
        // Create the set of candidate pending certificates as the set of all certificates minus the set of the committed certificates.
        let mut candidate_pending_certificates: IndexSet<BatchCertificate<CurrentNetwork>> =
            IndexSet::new();
        for certificate in certificates.clone() {
            if !committed_certificates.contains(&certificate) {
                candidate_pending_certificates.insert(certificate);
            }
        }
        // Check that the set of pending certificates is equal to the set of candidate pending certificates.
        assert_eq!(pending_certificates, candidate_pending_certificates);
        Ok(())
    }
}
