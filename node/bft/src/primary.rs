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
    DEVELOPMENT_MODE_RNG_SEED,
    MAX_BATCH_DELAY_IN_MS,
    MAX_WORKERS,
    MIN_BATCH_DELAY_IN_SECS,
    Sync,
    Worker,
    helpers::{
        BFTSender,
        PrimaryReceiver,
        PrimarySender,
        Proposal,
        ProposalCache,
        SignedProposals,
        Storage,
        assign_to_worker,
        assign_to_workers,
        fmt_id,
        now,
    },
    spawn_blocking,
};
use amareleo_chain_account::Account;
use amareleo_chain_tracing::{TracingHandler, TracingHandlerGuard};
use amareleo_node_bft_ledger_service::LedgerService;
use amareleo_node_sync::DUMMY_SELF_IP;
use snarkvm::{
    console::{prelude::*, types::Address},
    ledger::{
        block::Transaction,
        narwhal::{BatchCertificate, BatchHeader, Data, Transmission, TransmissionID},
        puzzle::{Solution, SolutionID},
    },
    prelude::{Signature, committee::Committee},
};

use aleo_std::StorageMode;
use colored::Colorize;
use futures::stream::{FuturesUnordered, StreamExt};
use indexmap::IndexMap;
#[cfg(feature = "locktick")]
use locktick::{
    parking_lot::{Mutex, RwLock},
    tokio::Mutex as TMutex,
};
#[cfg(not(feature = "locktick"))]
use parking_lot::{Mutex, RwLock};
use rand::SeedableRng;
use rand_chacha::ChaChaRng;
use snarkvm::console::account::PrivateKey;

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
#[cfg(not(feature = "locktick"))]
use tokio::sync::Mutex as TMutex;
use tokio::{sync::OnceCell, task::JoinHandle};
use tracing::subscriber::DefaultGuard;

/// A helper type for an optional proposed batch.
pub type ProposedBatch<N> = RwLock<Option<Proposal<N>>>;

#[derive(Clone)]
pub struct Primary<N: Network> {
    /// The sync module.
    sync: Sync<N>,
    /// Account
    account: Account<N>,
    /// The storage.
    storage: Storage<N>,
    /// Preserve the chain state on shutdown
    keep_state: bool,
    /// Set if primary is initialized and requires saving state on shutdown
    save_pending: Arc<AtomicBool>,
    /// The storage mode.
    storage_mode: StorageMode,
    /// The ledger service.
    ledger: Arc<dyn LedgerService<N>>,
    /// The workers.
    workers: Arc<[Worker<N>]>,
    /// The BFT sender.
    bft_sender: Arc<OnceCell<BFTSender<N>>>,
    /// The batch proposal, if the primary is currently proposing a batch.
    proposed_batch: Arc<ProposedBatch<N>>,
    /// The timestamp of the most recent proposed batch.
    latest_proposed_batch_timestamp: Arc<RwLock<i64>>,
    /// The recently-signed batch proposals.
    signed_proposals: Arc<RwLock<SignedProposals<N>>>,
    /// The spawned handles.
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
    /// Tracing handle
    tracing: Option<TracingHandler>,
    /// The lock for propose_batch.
    propose_lock: Arc<TMutex<u64>>,
}

impl<N: Network> TracingHandlerGuard for Primary<N> {
    /// Retruns tracing guard
    fn get_tracing_guard(&self) -> Option<DefaultGuard> {
        self.tracing.as_ref().and_then(|trace_handle| trace_handle.get_tracing_guard())
    }
}

impl<N: Network> Primary<N> {
    /// The maximum number of unconfirmed transmissions to send to the primary.
    pub const MAX_TRANSMISSIONS_TOLERANCE: usize = BatchHeader::<N>::MAX_TRANSMISSIONS_PER_BATCH * 2;

    /// Initializes a new primary instance.
    pub fn new(
        account: Account<N>,
        storage: Storage<N>,
        keep_state: bool,
        storage_mode: StorageMode,
        ledger: Arc<dyn LedgerService<N>>,
        tracing: Option<TracingHandler>,
    ) -> Result<Self> {
        // Initialize the sync module.
        let sync = Sync::new(storage.clone(), ledger.clone());

        // Initialize the primary instance.
        Ok(Self {
            sync,
            account,
            storage,
            keep_state,
            save_pending: Arc::new(AtomicBool::new(false)),
            storage_mode,
            ledger,
            workers: Arc::from(vec![]),
            bft_sender: Default::default(),
            proposed_batch: Default::default(),
            latest_proposed_batch_timestamp: Default::default(),
            signed_proposals: Default::default(),
            handles: Default::default(),
            tracing,
            propose_lock: Default::default(),
        })
    }

    /// Load the proposal cache file and update the Primary state with the stored data.
    async fn load_proposal_cache(&self) -> Result<()> {
        // Fetch the signed proposals from the file system if it exists.
        match ProposalCache::<N>::exists(&self.storage_mode) {
            // If the proposal cache exists, then process the proposal cache.
            true => match ProposalCache::<N>::load(self.account.address(), &self.storage_mode, self) {
                Ok(proposal_cache) => {
                    // Extract the proposal and signed proposals.
                    let (latest_certificate_round, proposed_batch, signed_proposals, pending_certificates) =
                        proposal_cache.into();

                    // Write the proposed batch.
                    *self.proposed_batch.write() = proposed_batch;
                    // Write the signed proposals.
                    *self.signed_proposals.write() = signed_proposals;
                    // Writ the propose lock.
                    *self.propose_lock.lock().await = latest_certificate_round;

                    // Update the storage with the pending certificates.
                    for certificate in pending_certificates {
                        let batch_id = certificate.batch_id();
                        // We use a dummy IP because the node should not need to request from any peers.
                        // The storage should have stored all the transmissions. If not, we simply
                        // skip the certificate.
                        if let Err(err) = self.sync_with_certificate_from_peer::<true>(DUMMY_SELF_IP, certificate).await
                        {
                            guard_warn!(
                                self,
                                "Failed to load stored certificate {} from proposal cache - {err}",
                                fmt_id(batch_id)
                            );
                        }
                    }
                    Ok(())
                }
                Err(err) => {
                    bail!("Failed to read the signed proposals from the file system - {err}.");
                }
            },
            // If the proposal cache does not exist, then return early.
            false => Ok(()),
        }
    }

    /// Run the primary instance.
    pub async fn run(
        &mut self,
        bft_sender: Option<BFTSender<N>>,
        _primary_sender: PrimarySender<N>,
        primary_receiver: PrimaryReceiver<N>,
    ) -> Result<()> {
        guard_info!(self, "Starting the primary instance of the memory pool...");

        // Set the BFT sender.
        if let Some(bft_sender) = &bft_sender {
            // Set the BFT sender in the primary.
            if self.bft_sender.set(bft_sender.clone()).is_err() {
                guard_error!(self, "Unexpected: BFT sender already set");
                bail!("Unexpected: BFT sender already set");
            }
        }

        // Construct a map for the workers.
        let mut workers = Vec::new();
        // Initialize the workers.
        for id in 0..MAX_WORKERS {
            // Construct the worker instance.
            let worker = Worker::new(
                id,
                self.storage.clone(),
                self.ledger.clone(),
                self.proposed_batch.clone(),
                self.tracing.clone(),
            )?;

            // Add the worker to the list of workers.
            workers.push(worker);
        }
        // Set the workers.
        self.workers = Arc::from(workers);

        // Next, initialize the sync module and sync the storage from ledger.
        self.sync.initialize(bft_sender).await?;
        // Next, load and process the proposal cache before running the sync module.
        self.load_proposal_cache().await?;
        // Next, run the sync module.
        self.sync.run().await?;

        // Lastly, start the primary handlers.
        // Note: This ensures the primary does not start communicating before syncing is complete.
        self.start_handlers(primary_receiver);

        // Once everything is initialized. Enable saving of state on shutdown.
        if self.keep_state {
            self.save_pending.store(true, Ordering::Relaxed);
        }

        Ok(())
    }

    /// Returns the current round.
    pub fn current_round(&self) -> u64 {
        self.storage.current_round()
    }

    /// Returns `true` if the primary is synced.
    pub fn is_synced(&self) -> bool {
        self.sync.is_synced()
    }

    /// Returns the account of the node.
    pub const fn account(&self) -> &Account<N> {
        &self.account
    }

    /// Returns the storage.
    pub const fn storage(&self) -> &Storage<N> {
        &self.storage
    }

    /// Returns the ledger.
    pub const fn ledger(&self) -> &Arc<dyn LedgerService<N>> {
        &self.ledger
    }

    /// Returns the number of workers.
    pub fn num_workers(&self) -> u8 {
        u8::try_from(self.workers.len()).expect("Too many workers")
    }
}

impl<N: Network> Primary<N> {
    /// Returns the number of unconfirmed transmissions.
    pub fn num_unconfirmed_transmissions(&self) -> usize {
        self.workers.iter().map(|worker| worker.num_transmissions()).sum()
    }

    /// Returns the number of unconfirmed ratifications.
    pub fn num_unconfirmed_ratifications(&self) -> usize {
        self.workers.iter().map(|worker| worker.num_ratifications()).sum()
    }

    /// Returns the number of solutions.
    pub fn num_unconfirmed_solutions(&self) -> usize {
        self.workers.iter().map(|worker| worker.num_solutions()).sum()
    }

    /// Returns the number of unconfirmed transactions.
    pub fn num_unconfirmed_transactions(&self) -> usize {
        self.workers.iter().map(|worker| worker.num_transactions()).sum()
    }
}

impl<N: Network> Primary<N> {
    /// Returns the worker transmission IDs.
    pub fn worker_transmission_ids(&self) -> impl '_ + Iterator<Item = TransmissionID<N>> {
        self.workers.iter().flat_map(|worker| worker.transmission_ids())
    }

    /// Returns the worker transmissions.
    pub fn worker_transmissions(&self) -> impl '_ + Iterator<Item = (TransmissionID<N>, Transmission<N>)> {
        self.workers.iter().flat_map(|worker| worker.transmissions())
    }

    /// Returns the worker solutions.
    pub fn worker_solutions(&self) -> impl '_ + Iterator<Item = (SolutionID<N>, Data<Solution<N>>)> {
        self.workers.iter().flat_map(|worker| worker.solutions())
    }

    /// Returns the worker transactions.
    pub fn worker_transactions(&self) -> impl '_ + Iterator<Item = (N::TransactionID, Data<Transaction<N>>)> {
        self.workers.iter().flat_map(|worker| worker.transactions())
    }
}

impl<N: Network> Primary<N> {
    /// Clears the worker solutions.
    pub fn clear_worker_solutions(&self) {
        self.workers.iter().for_each(Worker::clear_solutions);
    }
}

impl<N: Network> Primary<N> {
    pub async fn propose_batch(&self, complete: bool) -> Result<()> {
        let mut rng = ChaChaRng::seed_from_u64(DEVELOPMENT_MODE_RNG_SEED);
        let mut all_acc: Vec<Account<N>> = Vec::new();
        for _ in 0u64..4u64 {
            let private_key = PrivateKey::<N>::new(&mut rng)?;
            let acc = Account::<N>::try_from(private_key)?;
            all_acc.push(acc);
        }

        // Submit proposal for validator with id 0
        let other_acc: Vec<&Account<N>> = all_acc.iter().skip(1).collect();
        let round = self.propose_batch_lite(&other_acc, complete).await?;
        if !complete || round == 0u64 {
            return Ok(());
        }

        // Submit empty proposals for other validators
        for vid in 1u64..4u64 {
            let primary_acc = &all_acc[vid as usize];
            let other_acc: Vec<&Account<N>> =
                all_acc.iter().filter(|acc| acc.address() != primary_acc.address()).collect();

            self.fake_proposal(vid, primary_acc, &other_acc, round).await?;
        }
        Ok(())
    }

    pub async fn propose_batch_lite(&self, other_acc: &[&Account<N>], complete: bool) -> Result<u64> {
        // This function isn't re-entrant.
        let mut lock_guard = self.propose_lock.lock().await;

        // Retrieve the current round.
        let round = self.current_round();
        // Compute the previous round.
        let previous_round = round.saturating_sub(1);

        // If the current round is 0, return early.
        ensure!(round > 0, "Round 0 cannot have transaction batches");

        // If the current storage round is below the latest proposal round, then return early.
        if round < *lock_guard {
            guard_warn!(
                self,
                "Cannot propose a batch for round {round} - the latest proposal cache round is {}",
                *lock_guard
            );
            return Ok(0u64);
        }

        #[cfg(feature = "metrics")]
        metrics::gauge(metrics::bft::PROPOSAL_ROUND, round as f64);

        // Ensure that the primary does not create a new proposal too quickly.
        if let Err(e) = self.check_proposal_timestamp(previous_round, self.account.address(), now()) {
            guard_debug!(
                self,
                "Primary is safely skipping a batch proposal for round {round} - {}",
                format!("{e}").dimmed()
            );
            return Ok(0u64);
        }

        // Ensure the primary has not proposed a batch for this round before.
        if self.storage.contains_certificate_in_round_from(round, self.account.address()) {
            // If a BFT sender was provided, attempt to advance the current round.
            if let Some(bft_sender) = self.bft_sender.get() {
                match bft_sender.send_primary_round_to_bft(self.current_round()).await {
                    // 'is_ready' is true if the primary is ready to propose a batch for the next round.
                    Ok(true) => (), // continue,
                    // 'is_ready' is false if the primary is not ready to propose a batch for the next round.
                    Ok(false) => return Ok(0u64),
                    // An error occurred while attempting to advance the current round.
                    Err(e) => {
                        guard_warn!(self, "Failed to update the BFT to the next round - {e}");
                        return Err(e);
                    }
                }
            }
            guard_debug!(
                self,
                "Primary is safely skipping {}",
                format!("(round {round} was already certified)").dimmed()
            );
            return Ok(0u64);
        }

        // Determine if the current round has been proposed.
        // Note: Do NOT make this judgment in advance before rebroadcast and round update. Rebroadcasting is
        // good for network reliability and should not be prevented for the already existing proposed_batch.
        // If a certificate already exists for the current round, an attempt should be made to advance the
        // round as early as possible.
        if round == *lock_guard {
            guard_warn!(self, "Primary is safely skipping a batch proposal - round {round} already proposed");
            return Ok(0u64);
        }

        // Retrieve the committee to check against.
        let committee_lookback = self.ledger.get_committee_lookback_for_round(round)?;
        // Check if the primary is connected to enough validators to reach quorum threshold.
        {
            // Retrieve the connected validator addresses.
            let mut connected_validators: HashSet<Address<N>> = other_acc.iter().map(|acc| acc.address()).collect();

            // Append the primary to the set.
            connected_validators.insert(self.account.address());

            // If quorum threshold is not reached, return early.
            if !committee_lookback.is_quorum_threshold_reached(&connected_validators) {
                guard_debug!(
                    self,
                    "Primary is safely skipping a batch proposal for round {round} {}",
                    "(please connect to more validators)".dimmed()
                );
                guard_trace!(self, "Primary is connected to {} validators", connected_validators.len() - 1);
                return Ok(0u64);
            }
        }

        // Retrieve the previous certificates.
        let previous_certificates = self.storage.get_certificates_for_round(previous_round);

        // Check if the batch is ready to be proposed.
        // Note: The primary starts at round 1, and round 0 contains no certificates, by definition.
        let mut is_ready = previous_round == 0;
        // If the previous round is not 0, check if the previous certificates have reached the quorum threshold.
        if previous_round > 0 {
            // Retrieve the committee lookback for the round.
            let Ok(previous_committee_lookback) = self.ledger.get_committee_lookback_for_round(previous_round) else {
                bail!("Cannot propose a batch for round {round}: the committee lookback is not known yet")
            };
            // Construct a set over the authors.
            let authors = previous_certificates.iter().map(BatchCertificate::author).collect();
            // Check if the previous certificates have reached the quorum threshold.
            if previous_committee_lookback.is_quorum_threshold_reached(&authors) {
                is_ready = true;
            }
        }
        // If the batch is not ready to be proposed, return early.
        if !is_ready {
            guard_debug!(
                self,
                "Primary is safely skipping a batch proposal for round {round} {}",
                format!("(previous round {previous_round} has not reached quorum)").dimmed()
            );
            return Ok(0u64);
        }

        // Initialize the map of transmissions.
        let mut transmissions: IndexMap<_, _> = Default::default();
        // Track the total execution costs of the batch proposal as it is being constructed.
        let mut proposal_cost = 0u64;
        // Note: worker draining and transaction inclusion needs to be thought
        // through carefully when there is more than one worker. The fairness
        // provided by one worker (FIFO) is no longer guaranteed with multiple workers.
        debug_assert_eq!(MAX_WORKERS, 1);

        'outer: for worker in self.workers.iter() {
            let mut num_worker_transmissions = 0usize;

            while let Some((id, transmission)) = worker.remove_front() {
                // Check the selected transmissions are below the batch limit.
                if transmissions.len() >= BatchHeader::<N>::MAX_TRANSMISSIONS_PER_BATCH {
                    break 'outer;
                }

                // Check the max transmissions per worker is not exceeded.
                if num_worker_transmissions >= Worker::<N>::MAX_TRANSMISSIONS_PER_WORKER {
                    continue 'outer;
                }

                // Check if the ledger already contains the transmission.
                if self.ledger.contains_transmission(&id).unwrap_or(true) {
                    guard_trace!(self, "Proposing - Skipping transmission '{}' - Already in ledger", fmt_id(id));
                    continue;
                }

                // Check if the storage already contain the transmission.
                // Note: We do not skip if this is the first transmission in the proposal, to ensure that
                // the primary does not propose a batch with no transmissions.
                if !transmissions.is_empty() && self.storage.contains_transmission(id) {
                    guard_trace!(self, "Proposing - Skipping transmission '{}' - Already in storage", fmt_id(id));
                    continue;
                }

                // Check the transmission is still valid.
                match (id, transmission.clone()) {
                    (TransmissionID::Solution(solution_id, checksum), Transmission::Solution(solution)) => {
                        // Ensure the checksum matches. If not, skip the solution.
                        if !matches!(solution.to_checksum::<N>(), Ok(solution_checksum) if solution_checksum == checksum)
                        {
                            guard_trace!(
                                self,
                                "Proposing - Skipping solution '{}' - Checksum mismatch",
                                fmt_id(solution_id)
                            );
                            continue;
                        }
                        // Check if the solution is still valid.
                        if let Err(e) = self.ledger.check_solution_basic(solution_id, solution).await {
                            guard_trace!(self, "Proposing - Skipping solution '{}' - {e}", fmt_id(solution_id));
                            continue;
                        }
                    }
                    (TransmissionID::Transaction(transaction_id, checksum), Transmission::Transaction(transaction)) => {
                        // Ensure the checksum matches. If not, skip the transaction.
                        if !matches!(transaction.to_checksum::<N>(), Ok(transaction_checksum) if transaction_checksum == checksum )
                        {
                            guard_trace!(
                                self,
                                "Proposing - Skipping transaction '{}' - Checksum mismatch",
                                fmt_id(transaction_id)
                            );
                            continue;
                        }

                        // Deserialize the transaction. If the transaction exceeds the maximum size, then return an error.
                        let transaction = spawn_blocking!({
                            match transaction {
                                Data::Object(transaction) => Ok(transaction),
                                Data::Buffer(bytes) => {
                                    Ok(Transaction::<N>::read_le(&mut bytes.take(N::MAX_TRANSACTION_SIZE as u64))?)
                                }
                            }
                        })?;

                        // Check if the transaction is still valid.
                        // TODO: check if clone is cheap, otherwise fix.
                        if let Err(e) = self.ledger.check_transaction_basic(transaction_id, transaction.clone()).await {
                            guard_trace!(self, "Proposing - Skipping transaction '{}' - {e}", fmt_id(transaction_id));
                            continue;
                        }

                        // Compute the transaction spent cost (in microcredits).
                        // Note: We purposefully discard this transaction if we are unable to compute the spent cost.
                        let Ok(cost) = self.ledger.transaction_spent_cost_in_microcredits(transaction_id, transaction)
                        else {
                            guard_debug!(
                                self,
                                "Proposing - Skipping and discarding transaction '{}' - Unable to compute transaction spent cost",
                                fmt_id(transaction_id)
                            );
                            continue;
                        };

                        // Compute the next proposal cost.
                        // Note: We purposefully discard this transaction if the proposal cost overflows.
                        let Some(next_proposal_cost) = proposal_cost.checked_add(cost) else {
                            guard_debug!(
                                self,
                                "Proposing - Skipping and discarding transaction '{}' - Proposal cost overflowed",
                                fmt_id(transaction_id)
                            );
                            continue;
                        };

                        // Check if the next proposal cost exceeds the batch proposal spend limit.
                        if next_proposal_cost > BatchHeader::<N>::BATCH_SPEND_LIMIT {
                            guard_trace!(
                                self,
                                "Proposing - Skipping transaction '{}' - Batch spend limit surpassed ({next_proposal_cost} > {})",
                                fmt_id(transaction_id),
                                BatchHeader::<N>::BATCH_SPEND_LIMIT
                            );

                            // Reinsert the transmission into the worker.
                            worker.insert_front(id, transmission);
                            break 'outer;
                        }

                        // Update the proposal cost.
                        proposal_cost = next_proposal_cost;
                    }

                    // Note: We explicitly forbid including ratifications,
                    // as the protocol currently does not support ratifications.
                    (TransmissionID::Ratification, Transmission::Ratification) => continue,
                    // All other combinations are clearly invalid.
                    _ => continue,
                }

                // If the transmission is valid, insert it into the proposal's transmission list.
                transmissions.insert(id, transmission);
                num_worker_transmissions = num_worker_transmissions.saturating_add(1);
            }
        }

        // Determine the current timestamp.
        let current_timestamp = now();

        *lock_guard = round;

        /* Proceeding to sign & propose the batch. */
        guard_info!(self, "Proposing a batch with {} transmissions for round {round}...", transmissions.len());

        // Retrieve the private key.
        let private_key = *self.account.private_key();
        // Retrieve the committee ID.
        let committee_id = committee_lookback.id();
        // Prepare the transmission IDs.
        let transmission_ids = transmissions.keys().copied().collect();
        // Prepare the previous batch certificate IDs.
        let previous_certificate_ids = previous_certificates.into_iter().map(|c| c.id()).collect();
        // Sign the batch header and construct the proposal.
        let (batch_header, proposal) = spawn_blocking!(BatchHeader::new(
            &private_key,
            round,
            current_timestamp,
            committee_id,
            transmission_ids,
            previous_certificate_ids,
            &mut rand::thread_rng()
        ))
        .and_then(|batch_header| {
            Proposal::new(committee_lookback.clone(), batch_header.clone(), transmissions.clone())
                .map(|proposal| (batch_header, proposal))
        })
        .inspect_err(|_| {
            // On error, reinsert the transmissions and then propagate the error.
            if let Err(e) = self.reinsert_transmissions_into_workers(transmissions) {
                guard_error!(self, "Failed to reinsert transmissions: {e:?}");
            }
        })?;
        // Set the timestamp of the latest proposed batch.
        *self.latest_proposed_batch_timestamp.write() = proposal.timestamp();

        // // Set the proposed batch.
        // *self.proposed_batch.write() = Some(proposal);

        // Processing proposal
        if complete {
            self.complete_batch_lite(other_acc, round, batch_header, proposal, committee_lookback).await?;
        }

        Ok(round)
    }

    pub async fn complete_batch_lite(
        &self,
        other_acc: &[&Account<N>],
        round: u64,
        batch_header: BatchHeader<N>,
        mut proposal: Proposal<N>,
        committee_lookback: Committee<N>,
    ) -> Result<()> {
        guard_info!(self, "Quorum threshold reached - Preparing to certify our batch for round {round}...");

        // Retrieve the batch ID.
        let batch_id = batch_header.batch_id();

        // Forge signatures of other validators.
        for acc in other_acc.iter() {
            // Sign the batch ID.
            let signer_acc = (*acc).clone();
            let signer = signer_acc.address();
            let signature = spawn_blocking!(signer_acc.sign(&[batch_id], &mut rand::thread_rng()))?;

            // Add the signature to the batch.
            proposal.add_signature(signer, signature, &committee_lookback)?;
        }

        // Store the certified batch and broadcast it to all validators.
        // If there was an error storing the certificate, reinsert the transmissions back into the ready queue.
        if let Err(e) = self.store_and_broadcast_certificate_lite(&proposal, &committee_lookback).await {
            // Reinsert the transmissions back into the ready queue for the next proposal.
            self.reinsert_transmissions_into_workers(proposal.into_transmissions())?;
            return Err(e);
        }

        #[cfg(feature = "metrics")]
        metrics::increment_gauge(metrics::bft::CERTIFIED_BATCHES, 1.0);
        Ok(())
    }

    pub async fn fake_proposal(
        &self,
        vid: u64,
        primary_acc: &Account<N>,
        other_acc: &[&Account<N>],
        round: u64,
    ) -> Result<()> {
        let transmissions: IndexMap<_, _> = Default::default();
        let transmission_ids = transmissions.keys().copied().collect();

        let private_key = *primary_acc.private_key();
        let current_timestamp = now();

        let committee_lookback = self.ledger.get_committee_lookback_for_round(round)?;
        let committee_id = committee_lookback.id();

        let previous_round = round.saturating_sub(1);
        let previous_certificates = self.storage.get_certificates_for_round(previous_round);
        let previous_certificate_ids = previous_certificates.into_iter().map(|c| c.id()).collect();

        let (batch_header, mut proposal) = spawn_blocking!(BatchHeader::new(
            &private_key,
            round,
            current_timestamp,
            committee_id,
            transmission_ids,
            previous_certificate_ids,
            &mut rand::thread_rng()
        ))
        .and_then(|batch_header| {
            Proposal::new(committee_lookback.clone(), batch_header.clone(), transmissions.clone())
                .map(|proposal| (batch_header, proposal))
        })?;

        // Retrieve the batch ID.
        let batch_id = batch_header.batch_id();
        let mut our_sign: Option<Signature<N>> = None;

        // Forge signatures of other validators.
        for acc in other_acc.iter() {
            // Sign the batch ID.
            let signer_acc = (*acc).clone();
            let signer = signer_acc.address();
            let signature = spawn_blocking!(signer_acc.sign(&[batch_id], &mut rand::thread_rng()))?;

            if signer == self.account.address() {
                our_sign = Some(signature);
            }

            // Add the signature to the batch.
            proposal.add_signature(signer, signature, &committee_lookback)?;
        }

        // Ensure our signature was not inserted (validator 0 signature)
        let our_sign = match our_sign {
            Some(sign) => sign,
            None => bail!("Fake Proposal generation failed. Validator 0 signature missing."),
        };

        // Create the batch certificate and transmissions.
        let (certificate, transmissions) =
            tokio::task::block_in_place(|| proposal.to_certificate(&committee_lookback))?;

        // Convert the transmissions into a HashMap.
        // Note: Do not change the `Proposal` to use a HashMap. The ordering there is necessary for safety.
        let transmissions = transmissions.into_iter().collect::<HashMap<_, _>>();

        // Store the certified batch.
        let (storage, certificate_) = (self.storage.clone(), certificate.clone());
        spawn_blocking!(storage.insert_certificate(certificate_, transmissions, Default::default()))?;
        guard_info!(self, "Stored a batch certificate for validator/round {vid}/{round}");

        match self.signed_proposals.write().0.entry(primary_acc.address()) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                // If the validator has already signed a batch for this round, then return early,
                // since, if the peer still has not received the signature, they will request it again,
                // and the logic at the start of this function will resend the (now cached) signature
                // to the peer if asked to sign this batch proposal again.
                if entry.get().0 == round {
                    return Ok(());
                }
                // Otherwise, cache the round, batch ID, and signature for this validator.
                entry.insert((round, batch_id, our_sign));
                guard_info!(self, "Inserted signature to signed_proposals {vid}/{round}");
            }
            // If the validator has not signed a batch before, then continue.
            std::collections::hash_map::Entry::Vacant(entry) => {
                // Cache the round, batch ID, and signature for this validator.
                entry.insert((round, batch_id, our_sign));
                guard_info!(self, "Inserted signature to signed_proposals {vid}/{round}");
            }
        };

        if let Some(bft_sender) = self.bft_sender.get() {
            // Send the certificate to the BFT.
            if let Err(e) = bft_sender.send_primary_certificate_to_bft(certificate).await {
                guard_warn!(self, "Failed to update the BFT DAG from sync: {e}");
                return Err(e);
            };
        }

        Ok(())
    }
}

impl<N: Network> Primary<N> {
    /// Starts the primary handlers.
    fn start_handlers(&self, primary_receiver: PrimaryReceiver<N>) {
        let PrimaryReceiver { mut rx_unconfirmed_solution, mut rx_unconfirmed_transaction } = primary_receiver;

        // Start the batch proposer.
        let self_ = self.clone();
        self.spawn(async move {
            loop {
                // Sleep briefly, but longer than if there were no batch.
                tokio::time::sleep(Duration::from_millis(MAX_BATCH_DELAY_IN_MS)).await;
                let current_round = self_.current_round();
                // If the primary is not synced, then do not propose a batch.
                if !self_.is_synced() {
                    guard_debug!(
                        self_,
                        "Skipping batch proposal for round {current_round} {}",
                        "(node is syncing)".dimmed()
                    );
                    continue;
                }
                // A best-effort attempt to skip the scheduled batch proposal if
                // round progression already triggered one.
                if self_.propose_lock.try_lock().is_err() {
                    guard_trace!(
                        self_,
                        "Skipping batch proposal for round {current_round} {}",
                        "(node is already proposing)".dimmed()
                    );
                    continue;
                };
                // If there is no proposed batch, attempt to propose a batch.
                // Note: Do NOT spawn a task around this function call. Proposing a batch is a critical path,
                // and only one batch needs be proposed at a time.
                if let Err(e) = self_.propose_batch(true).await {
                    guard_warn!(self_, "Cannot propose a batch - {e}");
                }
            }
        });

        // Periodically try to increment to the next round.
        // Note: This is necessary to ensure that the primary is not stuck on a previous round
        // despite having received enough certificates to advance to the next round.
        let self_ = self.clone();
        self.spawn(async move {
            loop {
                // Sleep briefly.
                tokio::time::sleep(Duration::from_millis(MAX_BATCH_DELAY_IN_MS)).await;
                // If the primary is not synced, then do not increment to the next round.
                if !self_.is_synced() {
                    guard_trace!(self_, "Skipping round increment {}", "(node is syncing)".dimmed());
                    continue;
                }
                // Attempt to increment to the next round.
                let next_round = self_.current_round().saturating_add(1);
                // Determine if the quorum threshold is reached for the current round.
                let is_quorum_threshold_reached = {
                    // Retrieve the certificate authors for the next round.
                    let authors = self_.storage.get_certificate_authors_for_round(next_round);
                    // If there are no certificates, then skip this check.
                    if authors.is_empty() {
                        continue;
                    }
                    let Ok(committee_lookback) = self_.ledger.get_committee_lookback_for_round(next_round) else {
                        guard_warn!(self_, "Failed to retrieve the committee lookback for round {next_round}");
                        continue;
                    };
                    committee_lookback.is_quorum_threshold_reached(&authors)
                };
                // Attempt to increment to the next round if the quorum threshold is reached.
                if is_quorum_threshold_reached {
                    guard_debug!(self_, "Quorum threshold reached for round {}", next_round);
                    if let Err(e) = self_.try_increment_to_the_next_round(next_round).await {
                        guard_warn!(self_, "Failed to increment to the next round - {e}");
                    }
                }
            }
        });

        // Process the unconfirmed solutions.
        let self_ = self.clone();
        self.spawn(async move {
            while let Some((solution_id, solution, callback)) = rx_unconfirmed_solution.recv().await {
                // Compute the checksum for the solution.
                let Ok(checksum) = solution.to_checksum::<N>() else {
                    guard_error!(self_, "Failed to compute the checksum for the unconfirmed solution");
                    continue;
                };
                // Compute the worker ID.
                let Ok(worker_id) = assign_to_worker((solution_id, checksum), self_.num_workers()) else {
                    guard_error!(self_, "Unable to determine the worker ID for the unconfirmed solution");
                    continue;
                };

                // Retrieve the worker.
                let worker = &self_.workers[worker_id as usize];
                // Process the unconfirmed solution.
                let result = worker.process_unconfirmed_solution(solution_id, solution).await;
                // Send the result to the callback.
                callback.send(result).ok();
            }
        });

        // Process the unconfirmed transactions.
        let self_ = self.clone();
        self.spawn(async move {
            while let Some((transaction_id, transaction, callback)) = rx_unconfirmed_transaction.recv().await {
                guard_trace!(self_, "Primary - Received an unconfirmed transaction '{}'", fmt_id(transaction_id));
                // Compute the checksum for the transaction.
                let Ok(checksum) = transaction.to_checksum::<N>() else {
                    guard_error!(self_, "Failed to compute the checksum for the unconfirmed transaction");
                    continue;
                };
                // Compute the worker ID.
                let Ok(worker_id) = assign_to_worker::<N>((&transaction_id, &checksum), self_.num_workers()) else {
                    guard_error!(self_, "Unable to determine the worker ID for the unconfirmed transaction");
                    continue;
                };

                // Retrieve the worker.
                let worker = &self_.workers[worker_id as usize];
                // Process the unconfirmed transaction.
                let result = worker.process_unconfirmed_transaction(transaction_id, transaction).await;
                // Send the result to the callback.
                callback.send(result).ok();
            }
        });
    }

    /// Increments to the next round.
    async fn try_increment_to_the_next_round(&self, next_round: u64) -> Result<()> {
        // If the next round is within GC range, then iterate to the penultimate round.
        if self.current_round() + self.storage.max_gc_rounds() >= next_round {
            let mut fast_forward_round = self.current_round();
            // Iterate until the penultimate round is reached.
            while fast_forward_round < next_round.saturating_sub(1) {
                // Update to the next round in storage.
                fast_forward_round = self.storage.increment_to_next_round(fast_forward_round)?;
                // Clear the proposed batch.
                *self.proposed_batch.write() = None;
            }
        }

        // Retrieve the current round.
        let current_round = self.current_round();
        // Attempt to advance to the next round.
        if current_round < next_round {
            // If a BFT sender was provided, send the current round to the BFT.
            let is_ready = if let Some(bft_sender) = self.bft_sender.get() {
                match bft_sender.send_primary_round_to_bft(current_round).await {
                    Ok(is_ready) => is_ready,
                    Err(e) => {
                        guard_warn!(self, "Failed to update the BFT to the next round - {e}");
                        return Err(e);
                    }
                }
            }
            // Otherwise, handle the Narwhal case.
            else {
                // Update to the next round in storage.
                self.storage.increment_to_next_round(current_round)?;
                // Set 'is_ready' to 'true'.
                true
            };

            // Log whether the next round is ready.
            match is_ready {
                true => guard_debug!(self, "Primary is ready to propose the next round"),
                false => guard_debug!(self, "Primary is not ready to propose the next round"),
            }

            // If the node is ready, propose a batch for the next round.
            if is_ready {
                self.propose_batch(true).await?;
            }
        }
        Ok(())
    }

    /// Increments to the next round.
    async fn try_increment_to_the_next_round_lite(&self, next_round: u64) -> Result<()> {
        // If the next round is within GC range, then iterate to the penultimate round.
        if self.current_round() + self.storage.max_gc_rounds() >= next_round {
            let mut fast_forward_round = self.current_round();
            // Iterate until the penultimate round is reached.
            while fast_forward_round < next_round.saturating_sub(1) {
                // Update to the next round in storage.
                fast_forward_round = self.storage.increment_to_next_round(fast_forward_round)?;
                // Clear the proposed batch.
                *self.proposed_batch.write() = None;
            }
        }

        // Retrieve the current round.
        let current_round = self.current_round();
        // Attempt to advance to the next round.
        if current_round < next_round {
            // If a BFT sender was provided, send the current round to the BFT.
            let is_ready = if let Some(bft_sender) = self.bft_sender.get() {
                match bft_sender.send_primary_round_to_bft(current_round).await {
                    Ok(is_ready) => is_ready,
                    Err(e) => {
                        guard_warn!(self, "Failed to update the BFT to the next round - {e}");
                        return Err(e);
                    }
                }
            }
            // Otherwise, handle the Narwhal case.
            else {
                // Update to the next round in storage.
                self.storage.increment_to_next_round(current_round)?;
                // Set 'is_ready' to 'true'.
                true
            };

            // Log whether the next round is ready.
            match is_ready {
                true => guard_debug!(self, "Primary is ready to propose the next round"),
                false => guard_debug!(self, "Primary is not ready to propose the next round"),
            }

            // // If the node is ready, propose a batch for the next round.
            // if is_ready {
            //     self.propose_batch(true).await?;
            // }
        }
        Ok(())
    }

    /// Ensure the primary is not creating batch proposals too frequently.
    /// This checks that the certificate timestamp for the previous round is within the expected range.
    fn check_proposal_timestamp(&self, previous_round: u64, author: Address<N>, timestamp: i64) -> Result<()> {
        // Retrieve the timestamp of the previous timestamp to check against.
        let previous_timestamp = match self.storage.get_certificate_for_round_with_author(previous_round, author) {
            // Ensure that the previous certificate was created at least `MIN_BATCH_DELAY_IN_MS` seconds ago.
            Some(certificate) => certificate.timestamp(),
            None => *self.latest_proposed_batch_timestamp.read(),
        };

        // Determine the elapsed time since the previous timestamp.
        let elapsed = timestamp
            .checked_sub(previous_timestamp)
            .ok_or_else(|| anyhow!("Timestamp cannot be before the previous certificate at round {previous_round}"))?;
        // Ensure that the previous certificate was created at least `MIN_BATCH_DELAY_IN_MS` seconds ago.
        match elapsed < MIN_BATCH_DELAY_IN_SECS as i64 {
            true => bail!("Timestamp is too soon after the previous certificate at round {previous_round}"),
            false => Ok(()),
        }
    }

    /// Stores the certified batch and broadcasts it to all validators, returning the certificate.
    async fn store_and_broadcast_certificate_lite(
        &self,
        proposal: &Proposal<N>,
        committee: &Committee<N>,
    ) -> Result<()> {
        // Create the batch certificate and transmissions.
        let (certificate, transmissions) = tokio::task::block_in_place(|| proposal.to_certificate(committee))?;
        // Convert the transmissions into a HashMap.
        // Note: Do not change the `Proposal` to use a HashMap. The ordering there is necessary for safety.
        let transmissions = transmissions.into_iter().collect::<HashMap<_, _>>();
        // Store the certified batch.
        let (storage, certificate_) = (self.storage.clone(), certificate.clone());
        spawn_blocking!(storage.insert_certificate(certificate_, transmissions, Default::default()))?;
        guard_debug!(self, "Stored a batch certificate for round {}", certificate.round());
        // If a BFT sender was provided, send the certificate to the BFT.
        if let Some(bft_sender) = self.bft_sender.get() {
            // Await the callback to continue.
            if let Err(e) = bft_sender.send_primary_certificate_to_bft(certificate.clone()).await {
                guard_warn!(self, "Failed to update the BFT DAG from primary - {e}");
                return Err(e);
            };
        }
        // Log the certified batch.
        let num_transmissions = certificate.transmission_ids().len();
        let round = certificate.round();
        guard_info!(self, "\n\nOur batch with {num_transmissions} transmissions for round {round} was certified!\n");
        // Increment to the next round.
        self.try_increment_to_the_next_round_lite(round + 1).await
    }

    /// Stores the certified batch and broadcasts it to all validators, returning the certificate.
    /// Re-inserts the transmissions from the proposal into the workers.
    fn reinsert_transmissions_into_workers(
        &self,
        transmissions: IndexMap<TransmissionID<N>, Transmission<N>>,
    ) -> Result<()> {
        // Re-insert the transmissions into the workers.
        assign_to_workers(&self.workers, transmissions.into_iter(), |worker, transmission_id, transmission| {
            worker.reinsert(transmission_id, transmission);
        })
    }

    /// Recursively stores a given batch certificate, after ensuring:
    ///   - Ensure the round matches the committee round.
    ///   - Ensure the address is a member of the committee.
    ///   - Ensure the timestamp is within range.
    ///   - Ensure we have all of the transmissions.
    ///   - Ensure we have all of the previous certificates.
    ///   - Ensure the previous certificates are for the previous round (i.e. round - 1).
    ///   - Ensure the previous certificates have reached the quorum threshold.
    ///   - Ensure we have not already signed the batch ID.
    #[async_recursion::async_recursion]
    async fn sync_with_certificate_from_peer<const IS_SYNCING: bool>(
        &self,
        peer_ip: SocketAddr,
        certificate: BatchCertificate<N>,
    ) -> Result<()> {
        // Retrieve the batch header.
        let batch_header = certificate.batch_header();
        // Retrieve the batch round.
        let batch_round = batch_header.round();

        // If the certificate round is outdated, do not store it.
        if batch_round <= self.storage.gc_round() {
            return Ok(());
        }
        // If the certificate already exists in storage, return early.
        if self.storage.contains_certificate(certificate.id()) {
            return Ok(());
        }

        // If node is not in sync mode and the node is not synced. Then return an error.
        if !IS_SYNCING && !self.is_synced() {
            bail!(
                "Failed to process certificate `{}` at round {batch_round} from '{peer_ip}' (node is syncing)",
                fmt_id(certificate.id())
            );
        }

        // If the peer is ahead, use the batch header to sync up to the peer.
        let missing_transmissions = self.sync_with_batch_header_from_peer::<IS_SYNCING>(peer_ip, batch_header).await?;

        // Check if the certificate needs to be stored.
        if !self.storage.contains_certificate(certificate.id()) {
            // Store the batch certificate.
            let (storage, certificate_) = (self.storage.clone(), certificate.clone());
            spawn_blocking!(storage.insert_certificate(certificate_, missing_transmissions, Default::default()))?;
            guard_debug!(self, "Stored a batch certificate for round {batch_round} from '{peer_ip}'");
            // If a BFT sender was provided, send the round and certificate to the BFT.
            if let Some(bft_sender) = self.bft_sender.get() {
                // Send the certificate to the BFT.
                if let Err(e) = bft_sender.send_primary_certificate_to_bft(certificate).await {
                    guard_warn!(self, "Failed to update the BFT DAG from sync: {e}");
                    return Err(e);
                };
            }
        }
        Ok(())
    }

    /// Recursively syncs using the given batch header.
    async fn sync_with_batch_header_from_peer<const IS_SYNCING: bool>(
        &self,
        peer_ip: SocketAddr,
        batch_header: &BatchHeader<N>,
    ) -> Result<HashMap<TransmissionID<N>, Transmission<N>>> {
        // Retrieve the batch round.
        let batch_round = batch_header.round();

        // If the certificate round is outdated, do not store it.
        if batch_round <= self.storage.gc_round() {
            bail!("Round {batch_round} is too far in the past")
        }

        // If node is not in sync mode and the node is not synced. Then return an error.
        if !IS_SYNCING && !self.is_synced() {
            bail!(
                "Failed to process batch header `{}` at round {batch_round} from '{peer_ip}' (node is syncing)",
                fmt_id(batch_header.batch_id())
            );
        }

        // Determine if quorum threshold is reached on the batch round.
        let is_quorum_threshold_reached = {
            let authors = self.storage.get_certificate_authors_for_round(batch_round);
            let committee_lookback = self.ledger.get_committee_lookback_for_round(batch_round)?;
            committee_lookback.is_quorum_threshold_reached(&authors)
        };

        // Check if our primary should move to the next round.
        // Note: Checking that quorum threshold is reached is important for mitigating a race condition,
        // whereby Narwhal requires N-f, however the BFT only requires f+1. Without this check, the primary
        // will advance to the next round assuming f+1, not N-f, which can lead to a network stall.
        let is_behind_schedule = is_quorum_threshold_reached && batch_round > self.current_round();
        // Check if our primary is far behind the peer.
        let is_peer_far_in_future = batch_round > self.current_round() + self.storage.max_gc_rounds();
        // If our primary is far behind the peer, update our committee to the batch round.
        if is_behind_schedule || is_peer_far_in_future {
            // If the batch round is greater than the current committee round, update the committee.
            self.try_increment_to_the_next_round(batch_round).await?;
        }

        // Ensure the primary has all of the transmissions.
        let missing_transmissions = self.fetch_missing_transmissions(batch_header).await.map_err(|e| {
            anyhow!("Failed to fetch missing transmissions for round {batch_round} from '{peer_ip}' - {e}")
        })?;

        Ok(missing_transmissions)
    }

    /// Fetches any missing transmissions for the specified batch header.
    /// If a transmission does not exist, it will be fetched from the specified peer IP.
    async fn fetch_missing_transmissions(
        &self,
        batch_header: &BatchHeader<N>,
    ) -> Result<HashMap<TransmissionID<N>, Transmission<N>>> {
        // If the round is <= the GC round, return early.
        if batch_header.round() <= self.storage.gc_round() {
            return Ok(Default::default());
        }

        // Ensure this batch ID is new, otherwise return early.
        if self.storage.contains_batch(batch_header.batch_id()) {
            guard_trace!(self, "Batch for round {} from peer has already been processed", batch_header.round());
            return Ok(Default::default());
        }

        // Retrieve the workers.
        let workers = self.workers.clone();

        // Initialize a list for the transmissions.
        let mut fetch_transmissions = FuturesUnordered::new();

        // Retrieve the number of workers.
        let num_workers = self.num_workers();
        // Iterate through the transmission IDs.
        for transmission_id in batch_header.transmission_ids() {
            // If the transmission does not exist in storage, proceed to fetch the transmission.
            if !self.storage.contains_transmission(*transmission_id) {
                // Determine the worker ID.
                let Ok(worker_id) = assign_to_worker(*transmission_id, num_workers) else {
                    bail!("Unable to assign transmission ID '{transmission_id}' to a worker")
                };
                // Retrieve the worker.
                let Some(worker) = workers.get(worker_id as usize) else { bail!("Unable to find worker {worker_id}") };
                // Push the callback onto the list.
                fetch_transmissions.push(worker.get_or_fetch_transmission(*transmission_id));
            }
        }

        // Initialize a set for the transmissions.
        let mut transmissions = HashMap::with_capacity(fetch_transmissions.len());
        // Wait for all of the transmissions to be fetched.
        while let Some(result) = fetch_transmissions.next().await {
            // Retrieve the transmission.
            let (transmission_id, transmission) = result?;
            // Insert the transmission into the set.
            transmissions.insert(transmission_id, transmission);
        }
        // Return the transmissions.
        Ok(transmissions)
    }
}

impl<N: Network> Primary<N> {
    /// Spawns a task with the given future; it should only be used for long-running tasks.
    fn spawn<T: Future<Output = ()> + Send + 'static>(&self, future: T) {
        self.handles.lock().push(tokio::spawn(future));
    }

    /// Shuts down the primary.
    pub async fn shut_down(&self) {
        guard_info!(self, "Shutting down the primary...");
        {
            // Abort all primary tasks and clear the handles.
            let mut handles = self.handles.lock();
            handles.iter().for_each(|handle| handle.abort());
            handles.clear();
        }

        // Save the current proposal cache to disk,
        // and clear save_pending ensuring this
        // operation is only ran once.
        let save_pending = self.save_pending.swap(false, Ordering::AcqRel);
        if save_pending {
            let proposal_cache = {
                let proposal = self.proposed_batch.write().take();
                let signed_proposals = self.signed_proposals.read().clone();
                let latest_round = proposal.as_ref().map(Proposal::round).unwrap_or(*self.propose_lock.lock().await);
                let pending_certificates = self.storage.get_pending_certificates();
                ProposalCache::new(latest_round, proposal, signed_proposals, pending_certificates)
            };

            if let Err(err) = proposal_cache.store(&self.storage_mode, self) {
                guard_error!(self, "Failed to store the current proposal cache: {err}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amareleo_node_bft_ledger_service::MockLedgerService;
    use amareleo_node_bft_storage_service::BFTMemoryService;
    use snarkvm::{
        console::types::Field,
        ledger::{
            committee::{Committee, MIN_VALIDATOR_STAKE},
            ledger_test_helpers::sample_execution_transaction_with_fee,
        },
        prelude::{Address, Signature},
    };

    use bytes::Bytes;
    use indexmap::IndexSet;
    use rand::RngCore;

    type CurrentNetwork = snarkvm::prelude::MainnetV0;

    fn sample_committee(rng: &mut TestRng) -> (Vec<(SocketAddr, Account<CurrentNetwork>)>, Committee<CurrentNetwork>) {
        // Create a committee containing the primary's account.
        const COMMITTEE_SIZE: usize = 4;
        let mut rng_validator = ChaChaRng::seed_from_u64(DEVELOPMENT_MODE_RNG_SEED);
        let mut accounts = Vec::with_capacity(COMMITTEE_SIZE);
        let mut members = IndexMap::new();

        for i in 0..COMMITTEE_SIZE {
            let socket_addr = format!("127.0.0.1:{}", 5000 + i).parse().unwrap();
            let account = Account::new(&mut rng_validator).unwrap();

            members.insert(account.address(), (MIN_VALIDATOR_STAKE, true, rng.gen_range(0..100)));
            accounts.push((socket_addr, account));
        }

        (accounts, Committee::<CurrentNetwork>::new(1, members).unwrap())
    }

    // Returns a primary and a list of accounts in the configured committee.
    fn primary_with_committee(
        account_index: usize,
        accounts: &[(SocketAddr, Account<CurrentNetwork>)],
        committee: Committee<CurrentNetwork>,
        height: u32,
    ) -> Primary<CurrentNetwork> {
        let ledger = Arc::new(MockLedgerService::new_at_height(committee, height));
        let storage = Storage::new(ledger.clone(), Arc::new(BFTMemoryService::new()), 10, None);

        // Initialize the primary.
        let account = accounts[account_index].1.clone();
        let mut primary = Primary::new(account, storage, false, StorageMode::Development(0), ledger, None).unwrap();

        // Construct a worker instance.
        primary.workers = Arc::from([Worker::new(
            0, // id
            primary.storage.clone(),
            primary.ledger.clone(),
            primary.proposed_batch.clone(),
            None,
        )
        .unwrap()]);

        primary
    }

    fn primary_without_handlers(
        rng: &mut TestRng,
    ) -> (Primary<CurrentNetwork>, Vec<(SocketAddr, Account<CurrentNetwork>)>) {
        let (accounts, committee) = sample_committee(rng);
        let primary = primary_with_committee(
            0, // index of primary's account
            &accounts,
            committee,
            CurrentNetwork::CONSENSUS_HEIGHT(ConsensusVersion::V1).unwrap(),
        );

        (primary, accounts)
    }

    // Creates a mock solution.
    fn sample_unconfirmed_solution(rng: &mut TestRng) -> (SolutionID<CurrentNetwork>, Data<Solution<CurrentNetwork>>) {
        // Sample a random fake solution ID.
        let solution_id = rng.gen::<u64>().into();
        // Vary the size of the solutions.
        let size = rng.gen_range(1024..10 * 1024);
        // Sample random fake solution bytes.
        let mut vec = vec![0u8; size];
        rng.fill_bytes(&mut vec);
        let solution = Data::Buffer(Bytes::from(vec));
        // Return the solution ID and solution.
        (solution_id, solution)
    }

    // Samples a test transaction.
    fn sample_unconfirmed_transaction(
        rng: &mut TestRng,
    ) -> (<CurrentNetwork as Network>::TransactionID, Data<Transaction<CurrentNetwork>>) {
        let transaction = sample_execution_transaction_with_fee(false, rng);
        let id = transaction.id();

        (id, Data::Object(transaction))
    }

    // Creates a batch proposal with one solution and one transaction.
    fn create_test_proposal(
        author: &Account<CurrentNetwork>,
        committee: Committee<CurrentNetwork>,
        round: u64,
        previous_certificate_ids: IndexSet<Field<CurrentNetwork>>,
        timestamp: i64,
        num_transactions: u64,
        rng: &mut TestRng,
    ) -> Proposal<CurrentNetwork> {
        let mut transmission_ids = IndexSet::new();
        let mut transmissions = IndexMap::new();

        // Prepare the solution and insert into the sets.
        let (solution_id, solution) = sample_unconfirmed_solution(rng);
        let solution_checksum = solution.to_checksum::<CurrentNetwork>().unwrap();
        let solution_transmission_id = (solution_id, solution_checksum).into();
        transmission_ids.insert(solution_transmission_id);
        transmissions.insert(solution_transmission_id, Transmission::Solution(solution));

        // Prepare the transactions and insert into the sets.
        for _ in 0..num_transactions {
            let (transaction_id, transaction) = sample_unconfirmed_transaction(rng);
            let transaction_checksum = transaction.to_checksum::<CurrentNetwork>().unwrap();
            let transaction_transmission_id = (&transaction_id, &transaction_checksum).into();
            transmission_ids.insert(transaction_transmission_id);
            transmissions.insert(transaction_transmission_id, Transmission::Transaction(transaction));
        }

        // Retrieve the private key.
        let private_key = author.private_key();
        // Sign the batch header.
        let batch_header = BatchHeader::new(
            private_key,
            round,
            timestamp,
            committee.id(),
            transmission_ids,
            previous_certificate_ids,
            rng,
        )
        .unwrap();
        // Construct the proposal.
        Proposal::new(committee, batch_header, transmissions).unwrap()
    }

    /// Creates a signature of the batch ID for each committee member (excluding the primary).
    fn peer_signatures_for_batch(
        primary_address: Address<CurrentNetwork>,
        accounts: &[(SocketAddr, Account<CurrentNetwork>)],
        batch_id: Field<CurrentNetwork>,
        rng: &mut TestRng,
    ) -> IndexSet<Signature<CurrentNetwork>> {
        let mut signatures = IndexSet::new();
        for (_, account) in accounts {
            if account.address() == primary_address {
                continue;
            }
            let signature = account.sign(&[batch_id], rng).unwrap();
            signatures.insert(signature);
        }
        signatures
    }

    // Creates a batch certificate.
    fn create_batch_certificate(
        primary_address: Address<CurrentNetwork>,
        accounts: &[(SocketAddr, Account<CurrentNetwork>)],
        round: u64,
        previous_certificate_ids: IndexSet<Field<CurrentNetwork>>,
        rng: &mut TestRng,
    ) -> (BatchCertificate<CurrentNetwork>, HashMap<TransmissionID<CurrentNetwork>, Transmission<CurrentNetwork>>) {
        let timestamp = now();

        let author =
            accounts.iter().find(|&(_, acct)| acct.address() == primary_address).map(|(_, acct)| acct.clone()).unwrap();
        let private_key = author.private_key();

        let committee_id = Field::rand(rng);
        let (solution_id, solution) = sample_unconfirmed_solution(rng);
        let (transaction_id, transaction) = sample_unconfirmed_transaction(rng);
        let solution_checksum = solution.to_checksum::<CurrentNetwork>().unwrap();
        let transaction_checksum = transaction.to_checksum::<CurrentNetwork>().unwrap();

        let solution_transmission_id = (solution_id, solution_checksum).into();
        let transaction_transmission_id = (&transaction_id, &transaction_checksum).into();

        let transmission_ids = [solution_transmission_id, transaction_transmission_id].into();
        let transmissions = [
            (solution_transmission_id, Transmission::Solution(solution)),
            (transaction_transmission_id, Transmission::Transaction(transaction)),
        ]
        .into();

        let batch_header = BatchHeader::new(
            private_key,
            round,
            timestamp,
            committee_id,
            transmission_ids,
            previous_certificate_ids,
            rng,
        )
        .unwrap();
        let signatures = peer_signatures_for_batch(primary_address, accounts, batch_header.batch_id(), rng);
        let certificate = BatchCertificate::<CurrentNetwork>::from(batch_header, signatures).unwrap();
        (certificate, transmissions)
    }

    // Create a certificate chain up to round in primary storage.
    fn store_certificate_chain(
        primary: &Primary<CurrentNetwork>,
        accounts: &[(SocketAddr, Account<CurrentNetwork>)],
        round: u64,
        rng: &mut TestRng,
    ) -> IndexSet<Field<CurrentNetwork>> {
        let mut previous_certificates = IndexSet::<Field<CurrentNetwork>>::new();
        let mut next_certificates = IndexSet::<Field<CurrentNetwork>>::new();
        for cur_round in 1..round {
            for (_, account) in accounts.iter() {
                let (certificate, transmissions) = create_batch_certificate(
                    account.address(),
                    accounts,
                    cur_round,
                    previous_certificates.clone(),
                    rng,
                );
                next_certificates.insert(certificate.id());
                assert!(primary.storage.insert_certificate(certificate, transmissions, Default::default()).is_ok());
            }

            assert!(primary.storage.increment_to_next_round(cur_round).is_ok());
            previous_certificates = next_certificates;
            next_certificates = IndexSet::<Field<CurrentNetwork>>::new();
        }

        previous_certificates
    }

    #[tokio::test]
    async fn test_propose_batch() {
        let mut rng = TestRng::default();
        let (primary, _) = primary_without_handlers(&mut rng);

        // Check there is no batch currently proposed.
        assert!(primary.proposed_batch.read().is_none());

        // Generate a solution and a transaction.
        let (solution_id, solution) = sample_unconfirmed_solution(&mut rng);
        let (transaction_id, transaction) = sample_unconfirmed_transaction(&mut rng);

        // Store it on one of the workers.
        primary.workers[0].process_unconfirmed_solution(solution_id, solution).await.unwrap();
        primary.workers[0].process_unconfirmed_transaction(transaction_id, transaction).await.unwrap();

        // Try to propose a batch again. This time, it should succeed.
        assert!(primary.propose_batch(false).await.is_ok());

        // AlexZ: proposed_batch is no longer written to when processing new batches...
        // assert!(primary.proposed_batch.read().is_some());
    }

    #[tokio::test]
    async fn test_propose_batch_with_no_transmissions() {
        let mut rng = TestRng::default();
        let (primary, _) = primary_without_handlers(&mut rng);

        // Check there is no batch currently proposed.
        assert!(primary.proposed_batch.read().is_none());

        // Try to propose a batch with no transmissions.
        assert!(primary.propose_batch(false).await.is_ok());

        // AlexZ: proposed_batch is no longer written to when processing new batches...
        // assert!(primary.proposed_batch.read().is_some());
    }

    #[tokio::test]
    async fn test_propose_batch_in_round() {
        let round = 3;
        let mut rng = TestRng::default();
        let (primary, accounts) = primary_without_handlers(&mut rng);

        // Fill primary storage.
        store_certificate_chain(&primary, &accounts, round, &mut rng);

        // Sleep for a while to ensure the primary is ready to propose the next round.
        tokio::time::sleep(Duration::from_secs(MIN_BATCH_DELAY_IN_SECS)).await;

        // Generate a solution and a transaction.
        let (solution_id, solution) = sample_unconfirmed_solution(&mut rng);
        let (transaction_id, transaction) = sample_unconfirmed_transaction(&mut rng);

        // Store it on one of the workers.
        primary.workers[0].process_unconfirmed_solution(solution_id, solution).await.unwrap();
        primary.workers[0].process_unconfirmed_transaction(transaction_id, transaction).await.unwrap();

        // Propose a batch again. This time, it should succeed.
        assert!(primary.propose_batch(false).await.is_ok());

        // AlexZ: proposed_batch is no longer written to when processing new batches...
        // assert!(primary.proposed_batch.read().is_some());
    }

    #[tokio::test]
    async fn test_propose_batch_over_spend_limit() {
        let mut rng = TestRng::default();

        // Create a primary to test spend limit backwards compatibility with V4.
        let (accounts, committee) = sample_committee(&mut rng);
        let primary = primary_with_committee(
            0,
            &accounts,
            committee.clone(),
            CurrentNetwork::CONSENSUS_HEIGHT(ConsensusVersion::V4).unwrap(),
        );

        // Check there is no batch currently proposed.
        assert!(primary.proposed_batch.read().is_none());
        // Check the workers are empty.
        primary.workers.iter().for_each(|worker| assert!(worker.transmissions().is_empty()));

        // Generate a solution and a transaction.
        let (solution_id, solution) = sample_unconfirmed_solution(&mut rng);
        primary.workers[0].process_unconfirmed_solution(solution_id, solution).await.unwrap();

        for _i in 0..5 {
            let (transaction_id, transaction) = sample_unconfirmed_transaction(&mut rng);
            // Store it on one of the workers.
            primary.workers[0].process_unconfirmed_transaction(transaction_id, transaction).await.unwrap();
        }

        // Try to propose a batch again. This time, it should succeed.
        assert!(primary.propose_batch(false).await.is_ok());

        // AlexZ: proposed_batch is no longer written to when processing new batches...
        // Expect 2/5 transactions to be included in the proposal in addition to the solution.
        // assert_eq!(primary.proposed_batch.read().as_ref().unwrap().transmissions().len(), 3);

        // Check the transmissions were correctly drained from the workers.
        assert_eq!(primary.workers.iter().map(|worker| worker.transmissions().len()).sum::<usize>(), 3);
    }

    #[tokio::test]
    async fn test_propose_batch_with_storage_round_behind_proposal_lock() {
        let round = 3;
        let mut rng = TestRng::default();
        let (primary, _) = primary_without_handlers(&mut rng);

        // Check there is no batch currently proposed.
        assert!(primary.proposed_batch.read().is_none());

        // Generate a solution and a transaction.
        let (solution_id, solution) = sample_unconfirmed_solution(&mut rng);
        let (transaction_id, transaction) = sample_unconfirmed_transaction(&mut rng);

        // Store it on one of the workers.
        primary.workers[0].process_unconfirmed_solution(solution_id, solution).await.unwrap();
        primary.workers[0].process_unconfirmed_transaction(transaction_id, transaction).await.unwrap();

        // Set the proposal lock to a round ahead of the storage.
        let old_proposal_lock_round = *primary.propose_lock.lock().await;
        *primary.propose_lock.lock().await = round + 1;

        // Propose a batch and enforce that it fails.
        assert!(primary.propose_batch(false).await.is_ok());
        assert!(primary.proposed_batch.read().is_none());

        // Set the proposal lock back to the old round.
        *primary.propose_lock.lock().await = old_proposal_lock_round;

        // Try to propose a batch again. This time, it should succeed.
        assert!(primary.propose_batch(false).await.is_ok());

        // AlexZ: proposed_batch is no longer written to when processing new batches...
        // assert!(primary.proposed_batch.read().is_some());
    }

    #[tokio::test]
    async fn test_propose_batch_with_storage_round_behind_proposal() {
        let round = 5;
        let mut rng = TestRng::default();
        let (primary, accounts) = primary_without_handlers(&mut rng);

        // Generate previous certificates.
        let previous_certificates = store_certificate_chain(&primary, &accounts, round, &mut rng);

        // Create a valid proposal.
        let timestamp = now();
        let proposal = create_test_proposal(
            &primary.account,
            primary.ledger.current_committee().unwrap(),
            round + 1,
            previous_certificates,
            timestamp,
            1,
            &mut rng,
        );

        // Store the proposal on the primary.
        *primary.proposed_batch.write() = Some(proposal);

        // Try to propose a batch will terminate early because the storage is behind the proposal.
        assert!(primary.propose_batch(false).await.is_ok());
        assert!(primary.proposed_batch.read().is_some());
        assert!(primary.proposed_batch.read().as_ref().unwrap().round() > primary.current_round());
    }
}
