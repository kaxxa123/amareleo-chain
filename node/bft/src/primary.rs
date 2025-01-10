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
    helpers::{
        assign_to_worker, assign_to_workers, fmt_id, now, BFTSender, PrimaryReceiver,
        PrimarySender, Proposal, ProposalCache, SignedProposals, Storage,
    },
    spawn_blocking, Sync, Worker, MAX_BATCH_DELAY_IN_MS, MAX_WORKERS, MIN_BATCH_DELAY_IN_SECS,
};
use snarkos_lite_account::Account;
use snarkos_lite_node_bft_ledger_service::LedgerService;
use snarkos_lite_node_sync::DUMMY_SELF_IP;
use snarkvm::{
    console::{prelude::*, types::Address},
    ledger::{
        block::Transaction,
        narwhal::{BatchCertificate, BatchHeader, Data, Transmission, TransmissionID},
        puzzle::{Solution, SolutionID},
    },
    prelude::{committee::Committee, Signature},
};

use colored::Colorize;
use futures::stream::{FuturesUnordered, StreamExt};
use indexmap::IndexMap;
use parking_lot::{Mutex, RwLock};

// AlexZ: Needed for Validator to forge signatures.
use rand::SeedableRng;
use rand_chacha::ChaChaRng;
use snarkvm::console::account::PrivateKey;

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{Mutex as TMutex, OnceCell},
    task::JoinHandle,
};

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
    /// The lock for propose_batch.
    propose_lock: Arc<TMutex<u64>>,
}

impl<N: Network> Primary<N> {
    /// The maximum number of unconfirmed transmissions to send to the primary.
    pub const MAX_TRANSMISSIONS_TOLERANCE: usize =
        BatchHeader::<N>::MAX_TRANSMISSIONS_PER_BATCH * 2;

    /// Initializes a new primary instance.
    pub fn new(
        account: Account<N>,
        storage: Storage<N>,
        ledger: Arc<dyn LedgerService<N>>,
        _ip: Option<SocketAddr>,
        _trusted_validators: &[SocketAddr],
        _dev: Option<u16>,
    ) -> Result<Self> {
        // Initialize the sync module.
        let sync = Sync::new(storage.clone(), ledger.clone());

        // Initialize the primary instance.
        Ok(Self {
            sync,
            account,
            storage,
            ledger,
            workers: Arc::from(vec![]),
            bft_sender: Default::default(),
            proposed_batch: Default::default(),
            latest_proposed_batch_timestamp: Default::default(),
            signed_proposals: Default::default(),
            handles: Default::default(),
            propose_lock: Default::default(),
        })
    }

    /// Load the proposal cache file and update the Primary state with the stored data.
    async fn load_proposal_cache(&self) -> Result<()> {
        // Fetch the signed proposals from the file system if it exists.
        match ProposalCache::<N>::exists(Some(0u16)) {
            // If the proposal cache exists, then process the proposal cache.
            true => {
                match ProposalCache::<N>::load(self.account.address(), Some(0u16)) {
                    Ok(proposal_cache) => {
                        // Extract the proposal and signed proposals.
                        let (
                            latest_certificate_round,
                            proposed_batch,
                            signed_proposals,
                            pending_certificates,
                        ) = proposal_cache.into();

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
                            if let Err(err) = self
                                .sync_with_certificate_from_peer::<true>(DUMMY_SELF_IP, certificate)
                                .await
                            {
                                warn!("Failed to load stored certificate {} from proposal cache - {err}", fmt_id(batch_id));
                            }
                        }
                        Ok(())
                    }
                    Err(err) => {
                        bail!("Failed to read the signed proposals from the file system - {err}.");
                    }
                }
            }
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
        info!("Starting the primary instance of the memory pool...");

        // Set the BFT sender.
        if let Some(bft_sender) = &bft_sender {
            // Set the BFT sender in the primary.
            self.bft_sender
                .set(bft_sender.clone())
                .expect("BFT sender already set");
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

    /// Returns the workers.
    pub const fn workers(&self) -> &Arc<[Worker<N>]> {
        &self.workers
    }

    /// Returns the batch proposal of our primary, if one currently exists.
    pub fn proposed_batch(&self) -> &Arc<ProposedBatch<N>> {
        &self.proposed_batch
    }
}

impl<N: Network> Primary<N> {
    /// Returns the number of unconfirmed transmissions.
    pub fn num_unconfirmed_transmissions(&self) -> usize {
        self.workers
            .iter()
            .map(|worker| worker.num_transmissions())
            .sum()
    }

    /// Returns the number of unconfirmed ratifications.
    pub fn num_unconfirmed_ratifications(&self) -> usize {
        self.workers
            .iter()
            .map(|worker| worker.num_ratifications())
            .sum()
    }

    /// Returns the number of solutions.
    pub fn num_unconfirmed_solutions(&self) -> usize {
        self.workers
            .iter()
            .map(|worker| worker.num_solutions())
            .sum()
    }

    /// Returns the number of unconfirmed transactions.
    pub fn num_unconfirmed_transactions(&self) -> usize {
        self.workers
            .iter()
            .map(|worker| worker.num_transactions())
            .sum()
    }
}

impl<N: Network> Primary<N> {
    /// Returns the worker transmission IDs.
    pub fn worker_transmission_ids(&self) -> impl '_ + Iterator<Item = TransmissionID<N>> {
        self.workers
            .iter()
            .flat_map(|worker| worker.transmission_ids())
    }

    /// Returns the worker transmissions.
    pub fn worker_transmissions(
        &self,
    ) -> impl '_ + Iterator<Item = (TransmissionID<N>, Transmission<N>)> {
        self.workers
            .iter()
            .flat_map(|worker| worker.transmissions())
    }

    /// Returns the worker solutions.
    pub fn worker_solutions(
        &self,
    ) -> impl '_ + Iterator<Item = (SolutionID<N>, Data<Solution<N>>)> {
        self.workers.iter().flat_map(|worker| worker.solutions())
    }

    /// Returns the worker transactions.
    pub fn worker_transactions(
        &self,
    ) -> impl '_ + Iterator<Item = (N::TransactionID, Data<Transaction<N>>)> {
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
    pub async fn propose_batch(&self) -> Result<()> {
        let mut rng = ChaChaRng::seed_from_u64(1234567890u64);
        let mut all_acc: Vec<Account<N>> = Vec::new();

        for _ in 0u64..4u64 {
            let private_key = PrivateKey::<N>::new(&mut rng)?;
            let acc = Account::<N>::try_from(private_key)
                .expect("Failed to initialize account with private key");
            all_acc.push(acc);
        }

        // Submit proposal for validator with id 0
        let primary_addr = all_acc[0].address();
        let other_acc: Vec<&Account<N>> = all_acc
            .iter()
            .filter(|acc| acc.address() != primary_addr)
            .collect();

        let round = self.propose_batch_lite(&other_acc).await?;
        if round == 0u64 {
            return Ok(());
        }

        // Submit empty proposals for other validators
        for vid in 1..all_acc.len() {
            let primary_acc = &all_acc[vid];
            let other_acc: Vec<&Account<N>> = all_acc
                .iter()
                .filter(|acc| acc.address() != primary_acc.address())
                .collect();

            self.fake_proposal(vid.try_into().unwrap(), primary_acc, &other_acc, round)
                .await?;
        }
        Ok(())
    }

    pub async fn propose_batch_lite(&self, other_acc: &[&Account<N>]) -> Result<u64> {
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
            warn!(
                "Cannot propose a batch for round {round} - the latest proposal cache round is {}",
                *lock_guard
            );
            return Ok(0u64);
        }

        #[cfg(feature = "metrics")]
        metrics::gauge(metrics::bft::PROPOSAL_ROUND, round as f64);

        // Ensure that the primary does not create a new proposal too quickly.
        if let Err(e) = self.check_proposal_timestamp(previous_round, self.account.address(), now())
        {
            debug!(
                "Primary is safely skipping a batch proposal - {}",
                format!("{e}").dimmed()
            );
            return Ok(0u64);
        }

        // Ensure the primary has not proposed a batch for this round before.
        if self
            .storage
            .contains_certificate_in_round_from(round, self.account.address())
        {
            // If a BFT sender was provided, attempt to advance the current round.
            if let Some(bft_sender) = self.bft_sender.get() {
                match bft_sender
                    .send_primary_round_to_bft(self.current_round())
                    .await
                {
                    // 'is_ready' is true if the primary is ready to propose a batch for the next round.
                    Ok(true) => (), // continue,
                    // 'is_ready' is false if the primary is not ready to propose a batch for the next round.
                    Ok(false) => return Ok(0u64),
                    // An error occurred while attempting to advance the current round.
                    Err(e) => {
                        warn!("Failed to update the BFT to the next round - {e}");
                        return Err(e);
                    }
                }
            }
            debug!(
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
            warn!("Primary is safely skipping a batch proposal - round {round} already proposed");
            return Ok(0u64);
        }

        // Retrieve the committee to check against.
        let committee_lookback = self.ledger.get_committee_lookback_for_round(round)?;
        // Check if the primary is connected to enough validators to reach quorum threshold.
        {
            // Retrieve the connected validator addresses.
            let mut connected_validators: HashSet<Address<N>> =
                other_acc.iter().map(|acc| acc.address()).collect();

            // Append the primary to the set.
            connected_validators.insert(self.account.address());

            // If quorum threshold is not reached, return early.
            if !committee_lookback.is_quorum_threshold_reached(&connected_validators) {
                debug!(
                    "Primary is safely skipping a batch proposal {}",
                    "(please connect to more validators)".dimmed()
                );
                trace!(
                    "Primary is connected to {} validators",
                    connected_validators.len() - 1
                );
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
            let Ok(previous_committee_lookback) =
                self.ledger.get_committee_lookback_for_round(previous_round)
            else {
                bail!("Cannot propose a batch for round {round}: the committee lookback is not known yet")
            };
            // Construct a set over the authors.
            let authors = previous_certificates
                .iter()
                .map(BatchCertificate::author)
                .collect();
            // Check if the previous certificates have reached the quorum threshold.
            if previous_committee_lookback.is_quorum_threshold_reached(&authors) {
                is_ready = true;
            }
        }
        // If the batch is not ready to be proposed, return early.
        if !is_ready {
            debug!(
                "Primary is safely skipping a batch proposal {}",
                format!("(previous round {previous_round} has not reached quorum)").dimmed()
            );
            return Ok(0u64);
        }

        // Determined the required number of transmissions per worker.
        let num_transmissions_per_worker =
            BatchHeader::<N>::MAX_TRANSMISSIONS_PER_BATCH / self.num_workers() as usize;
        // Initialize the map of transmissions.
        let mut transmissions: IndexMap<_, _> = Default::default();
        // Take the transmissions from the workers.
        for worker in self.workers.iter() {
            // Initialize a tracker for included transmissions for the current worker.
            let mut num_transmissions_included_for_worker = 0;
            // Keep draining the worker until the desired number of transmissions is reached or the worker is empty.
            'outer: while num_transmissions_included_for_worker < num_transmissions_per_worker {
                // Determine the number of remaining transmissions for the worker.
                let num_remaining_transmissions = num_transmissions_per_worker
                    .saturating_sub(num_transmissions_included_for_worker);
                // Drain the worker.
                let mut worker_transmissions = worker.drain(num_remaining_transmissions).peekable();
                // If the worker is empty, break early.
                if worker_transmissions.peek().is_none() {
                    break 'outer;
                }
                // Iterate through the worker transmissions.
                'inner: for (id, transmission) in worker_transmissions {
                    // Check if the ledger already contains the transmission.
                    if self.ledger.contains_transmission(&id).unwrap_or(true) {
                        trace!(
                            "Proposing - Skipping transmission '{}' - Already in ledger",
                            fmt_id(id)
                        );
                        continue 'inner;
                    }
                    // Check if the storage already contain the transmission.
                    // Note: We do not skip if this is the first transmission in the proposal, to ensure that
                    // the primary does not propose a batch with no transmissions.
                    if !transmissions.is_empty() && self.storage.contains_transmission(id) {
                        trace!(
                            "Proposing - Skipping transmission '{}' - Already in storage",
                            fmt_id(id)
                        );
                        continue 'inner;
                    }
                    // Check the transmission is still valid.
                    match (id, transmission.clone()) {
                        (
                            TransmissionID::Solution(solution_id, checksum),
                            Transmission::Solution(solution),
                        ) => {
                            // Ensure the checksum matches.
                            match solution.to_checksum::<N>() {
                                Ok(solution_checksum) if solution_checksum == checksum => (),
                                _ => {
                                    trace!(
                                        "Proposing - Skipping solution '{}' - Checksum mismatch",
                                        fmt_id(solution_id)
                                    );
                                    continue 'inner;
                                }
                            }
                            // Check if the solution is still valid.
                            if let Err(e) = self
                                .ledger
                                .check_solution_basic(solution_id, solution)
                                .await
                            {
                                trace!(
                                    "Proposing - Skipping solution '{}' - {e}",
                                    fmt_id(solution_id)
                                );
                                continue 'inner;
                            }
                        }
                        (
                            TransmissionID::Transaction(transaction_id, checksum),
                            Transmission::Transaction(transaction),
                        ) => {
                            // Ensure the checksum matches.
                            match transaction.to_checksum::<N>() {
                                Ok(transaction_checksum) if transaction_checksum == checksum => (),
                                _ => {
                                    trace!(
                                        "Proposing - Skipping transaction '{}' - Checksum mismatch",
                                        fmt_id(transaction_id)
                                    );
                                    continue 'inner;
                                }
                            }
                            // Check if the transaction is still valid.
                            if let Err(e) = self
                                .ledger
                                .check_transaction_basic(transaction_id, transaction)
                                .await
                            {
                                trace!(
                                    "Proposing - Skipping transaction '{}' - {e}",
                                    fmt_id(transaction_id)
                                );
                                continue 'inner;
                            }
                        }
                        // Note: We explicitly forbid including ratifications,
                        // as the protocol currently does not support ratifications.
                        (TransmissionID::Ratification, Transmission::Ratification) => continue,
                        // All other combinations are clearly invalid.
                        _ => continue 'inner,
                    }
                    // Insert the transmission into the map.
                    transmissions.insert(id, transmission);
                    num_transmissions_included_for_worker += 1;
                }
            }
        }

        // Determine the current timestamp.
        let current_timestamp = now();

        *lock_guard = round;

        /* Proceeding to sign & propose the batch. */
        info!(
            "Proposing a batch with {} transmissions for round {round}...",
            transmissions.len()
        );

        // Retrieve the private key.
        let private_key = *self.account.private_key();
        // Retrieve the committee ID.
        let committee_id = committee_lookback.id();
        // Prepare the transmission IDs.
        let transmission_ids = transmissions.keys().copied().collect();
        // Prepare the previous batch certificate IDs.
        let previous_certificate_ids = previous_certificates.into_iter().map(|c| c.id()).collect();
        // Sign the batch header and construct the proposal.
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
            Proposal::new(
                committee_lookback.clone(),
                batch_header.clone(),
                transmissions.clone(),
            )
            .map(|proposal| (batch_header, proposal))
        })
        .inspect_err(|_| {
            // On error, reinsert the transmissions and then propagate the error.
            if let Err(e) = self.reinsert_transmissions_into_workers(transmissions) {
                error!("Failed to reinsert transmissions: {e:?}");
            }
        })?;
        // Set the timestamp of the latest proposed batch.
        *self.latest_proposed_batch_timestamp.write() = proposal.timestamp();

        // // Set the proposed batch.
        // *self.proposed_batch.write() = Some(proposal);

        //===============================================================================
        // Processing proposal

        info!("Quorum threshold reached - Preparing to certify our batch for round {round}...");

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
        if let Err(e) = self
            .store_and_broadcast_certificate_lite(&proposal, &committee_lookback)
            .await
        {
            // Reinsert the transmissions back into the ready queue for the next proposal.
            self.reinsert_transmissions_into_workers(proposal.into_transmissions())?;
            return Err(e);
        }

        #[cfg(feature = "metrics")]
        metrics::increment_gauge(metrics::bft::CERTIFIED_BATCHES, 1.0);
        Ok(round)
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
            Proposal::new(
                committee_lookback.clone(),
                batch_header.clone(),
                transmissions.clone(),
            )
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
        spawn_blocking!(storage.insert_certificate(
            certificate_,
            transmissions,
            Default::default()
        ))?;
        info!("Stored a batch certificate for validator/round {vid}/{round}");

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
                info!("Inserted signature to signed_proposals {vid}/{round}");
            }
            // If the validator has not signed a batch before, then continue.
            std::collections::hash_map::Entry::Vacant(entry) => {
                // Cache the round, batch ID, and signature for this validator.
                entry.insert((round, batch_id, our_sign));
                info!("Inserted signature to signed_proposals {vid}/{round}");
            }
        };

        if let Some(bft_sender) = self.bft_sender.get() {
            // Send the certificate to the BFT.
            if let Err(e) = bft_sender
                .send_primary_certificate_to_bft(certificate)
                .await
            {
                warn!("Failed to update the BFT DAG from sync: {e}");
                return Err(e);
            };
        }

        Ok(())
    }
}

impl<N: Network> Primary<N> {
    /// Starts the primary handlers.
    fn start_handlers(&self, primary_receiver: PrimaryReceiver<N>) {
        let PrimaryReceiver {
            rx_batch_propose: _,
            rx_batch_signature: _,
            rx_batch_certified: _,
            rx_primary_ping: _,
            mut rx_unconfirmed_solution,
            mut rx_unconfirmed_transaction,
        } = primary_receiver;

        // Start the batch proposer.
        let self_ = self.clone();
        self.spawn(async move {
            loop {
                // Sleep briefly, but longer than if there were no batch.
                tokio::time::sleep(Duration::from_millis(MAX_BATCH_DELAY_IN_MS)).await;
                // If the primary is not synced, then do not propose a batch.
                if !self_.is_synced() {
                    debug!("Skipping batch proposal {}", "(node is syncing)".dimmed());
                    continue;
                }
                // A best-effort attempt to skip the scheduled batch proposal if
                // round progression already triggered one.
                if self_.propose_lock.try_lock().is_err() {
                    trace!(
                        "Skipping batch proposal {}",
                        "(node is already proposing)".dimmed()
                    );
                    continue;
                };
                // If there is no proposed batch, attempt to propose a batch.
                // Note: Do NOT spawn a task around this function call. Proposing a batch is a critical path,
                // and only one batch needs be proposed at a time.
                if let Err(e) = self_.propose_batch().await {
                    warn!("Cannot propose a batch - {e}");
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
                    trace!("Skipping round increment {}", "(node is syncing)".dimmed());
                    continue;
                }
                // Attempt to increment to the next round.
                let next_round = self_.current_round().saturating_add(1);
                // Determine if the quorum threshold is reached for the current round.
                let is_quorum_threshold_reached = {
                    // Retrieve the certificates for the next round.
                    let certificates = self_.storage.get_certificates_for_round(next_round);
                    // If there are no certificates, then skip this check.
                    if certificates.is_empty() {
                        continue;
                    }
                    let Ok(committee_lookback) =
                        self_.ledger.get_committee_lookback_for_round(next_round)
                    else {
                        warn!("Failed to retrieve the committee lookback for round {next_round}");
                        continue;
                    };
                    let authors = certificates.iter().map(BatchCertificate::author).collect();
                    committee_lookback.is_quorum_threshold_reached(&authors)
                };
                // Attempt to increment to the next round if the quorum threshold is reached.
                if is_quorum_threshold_reached {
                    debug!("Quorum threshold reached for round {}", next_round);
                    if let Err(e) = self_.try_increment_to_the_next_round(next_round).await {
                        warn!("Failed to increment to the next round - {e}");
                    }
                }
            }
        });

        // Process the unconfirmed solutions.
        let self_ = self.clone();
        self.spawn(async move {
            while let Some((solution_id, solution, callback)) = rx_unconfirmed_solution.recv().await
            {
                // Compute the checksum for the solution.
                let Ok(checksum) = solution.to_checksum::<N>() else {
                    error!("Failed to compute the checksum for the unconfirmed solution");
                    continue;
                };
                // Compute the worker ID.
                let Ok(worker_id) = assign_to_worker((solution_id, checksum), self_.num_workers())
                else {
                    error!("Unable to determine the worker ID for the unconfirmed solution");
                    continue;
                };
                let self_ = self_.clone();
                tokio::spawn(async move {
                    // Retrieve the worker.
                    let worker = &self_.workers[worker_id as usize];
                    // Process the unconfirmed solution.
                    let result = worker
                        .process_unconfirmed_solution(solution_id, solution)
                        .await;
                    // Send the result to the callback.
                    callback.send(result).ok();
                });
            }
        });

        // Process the unconfirmed transactions.
        let self_ = self.clone();
        self.spawn(async move {
            while let Some((transaction_id, transaction, callback)) =
                rx_unconfirmed_transaction.recv().await
            {
                trace!(
                    "Primary - Received an unconfirmed transaction '{}'",
                    fmt_id(transaction_id)
                );
                // Compute the checksum for the transaction.
                let Ok(checksum) = transaction.to_checksum::<N>() else {
                    error!("Failed to compute the checksum for the unconfirmed transaction");
                    continue;
                };
                // Compute the worker ID.
                let Ok(worker_id) =
                    assign_to_worker::<N>((&transaction_id, &checksum), self_.num_workers())
                else {
                    error!("Unable to determine the worker ID for the unconfirmed transaction");
                    continue;
                };
                let self_ = self_.clone();
                tokio::spawn(async move {
                    // Retrieve the worker.
                    let worker = &self_.workers[worker_id as usize];
                    // Process the unconfirmed transaction.
                    let result = worker
                        .process_unconfirmed_transaction(transaction_id, transaction)
                        .await;
                    // Send the result to the callback.
                    callback.send(result).ok();
                });
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
                        warn!("Failed to update the BFT to the next round - {e}");
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
                true => debug!("Primary is ready to propose the next round"),
                false => debug!("Primary is not ready to propose the next round"),
            }

            // If the node is ready, propose a batch for the next round.
            if is_ready {
                self.propose_batch().await?;
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
                        warn!("Failed to update the BFT to the next round - {e}");
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
                true => debug!("Primary is ready to propose the next round"),
                false => debug!("Primary is not ready to propose the next round"),
            }

            // // If the node is ready, propose a batch for the next round.
            // if is_ready {
            //     self.propose_batch().await?;
            // }
        }
        Ok(())
    }

    /// Ensure the primary is not creating batch proposals too frequently.
    /// This checks that the certificate timestamp for the previous round is within the expected range.
    fn check_proposal_timestamp(
        &self,
        previous_round: u64,
        author: Address<N>,
        timestamp: i64,
    ) -> Result<()> {
        // Retrieve the timestamp of the previous timestamp to check against.
        let previous_timestamp = match self
            .storage
            .get_certificate_for_round_with_author(previous_round, author)
        {
            // Ensure that the previous certificate was created at least `MIN_BATCH_DELAY_IN_MS` seconds ago.
            Some(certificate) => certificate.timestamp(),

            // AlexZ: Function was handling special case for: self.account.address() == author
            //        Short-circuited to case when this is true.
            None => *self.latest_proposed_batch_timestamp.read(),
        };

        // Determine the elapsed time since the previous timestamp.
        let elapsed = timestamp.checked_sub(previous_timestamp).ok_or_else(|| {
            anyhow!("Timestamp cannot be before the previous certificate at round {previous_round}")
        })?;
        // Ensure that the previous certificate was created at least `MIN_BATCH_DELAY_IN_MS` seconds ago.
        match elapsed < MIN_BATCH_DELAY_IN_SECS as i64 {
            true => bail!(
                "Timestamp is too soon after the previous certificate at round {previous_round}"
            ),
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
        let (certificate, transmissions) =
            tokio::task::block_in_place(|| proposal.to_certificate(committee))?;
        // Convert the transmissions into a HashMap.
        // Note: Do not change the `Proposal` to use a HashMap. The ordering there is necessary for safety.
        let transmissions = transmissions.into_iter().collect::<HashMap<_, _>>();
        // Store the certified batch.
        let (storage, certificate_) = (self.storage.clone(), certificate.clone());
        spawn_blocking!(storage.insert_certificate(
            certificate_,
            transmissions,
            Default::default()
        ))?;
        debug!(
            "Stored a batch certificate for round {}",
            certificate.round()
        );
        // If a BFT sender was provided, send the certificate to the BFT.
        if let Some(bft_sender) = self.bft_sender.get() {
            // Await the callback to continue.
            if let Err(e) = bft_sender
                .send_primary_certificate_to_bft(certificate.clone())
                .await
            {
                warn!("Failed to update the BFT DAG from primary - {e}");
                return Err(e);
            };
        }
        // Log the certified batch.
        let num_transmissions = certificate.transmission_ids().len();
        let round = certificate.round();
        info!("\n\nOur batch with {num_transmissions} transmissions for round {round} was certified!\n");
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
        assign_to_workers(
            &self.workers,
            transmissions.into_iter(),
            |worker, transmission_id, transmission| {
                worker.reinsert(transmission_id, transmission);
            },
        )
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
        let missing_transmissions = self
            .sync_with_batch_header_from_peer::<IS_SYNCING>(peer_ip, batch_header)
            .await?;

        // Check if the certificate needs to be stored.
        if !self.storage.contains_certificate(certificate.id()) {
            // Store the batch certificate.
            let (storage, certificate_) = (self.storage.clone(), certificate.clone());
            spawn_blocking!(storage.insert_certificate(
                certificate_,
                missing_transmissions,
                Default::default()
            ))?;
            debug!("Stored a batch certificate for round {batch_round} from '{peer_ip}'");
            // If a BFT sender was provided, send the round and certificate to the BFT.
            if let Some(bft_sender) = self.bft_sender.get() {
                // Send the certificate to the BFT.
                if let Err(e) = bft_sender
                    .send_primary_certificate_to_bft(certificate)
                    .await
                {
                    warn!("Failed to update the BFT DAG from sync: {e}");
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
            let certificates = self.storage.get_certificates_for_round(batch_round);
            let authors = certificates.iter().map(BatchCertificate::author).collect();
            let committee_lookback = self.ledger.get_committee_lookback_for_round(batch_round)?;
            committee_lookback.is_quorum_threshold_reached(&authors)
        };

        // Check if our primary should move to the next round.
        // Note: Checking that quorum threshold is reached is important for mitigating a race condition,
        // whereby Narwhal requires N-f, however the BFT only requires f+1. Without this check, the primary
        // will advance to the next round assuming f+1, not N-f, which can lead to a network stall.
        let is_behind_schedule = is_quorum_threshold_reached && batch_round > self.current_round();
        // Check if our primary is far behind the peer.
        let is_peer_far_in_future =
            batch_round > self.current_round() + self.storage.max_gc_rounds();
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
            trace!(
                "Batch for round {} from peer has already been processed",
                batch_header.round()
            );
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
                let Some(worker) = workers.get(worker_id as usize) else {
                    bail!("Unable to find worker {worker_id}")
                };
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
        info!("Shutting down the primary...");
        // Abort the tasks.
        self.handles.lock().iter().for_each(|handle| handle.abort());
        // Save the current proposal cache to disk.
        let proposal_cache = {
            let proposal = self.proposed_batch.write().take();
            let signed_proposals = self.signed_proposals.read().clone();
            let latest_round = proposal
                .as_ref()
                .map(Proposal::round)
                .unwrap_or(*self.propose_lock.lock().await);
            let pending_certificates = self.storage.get_pending_certificates();
            ProposalCache::new(
                latest_round,
                proposal,
                signed_proposals,
                pending_certificates,
            )
        };
        if let Err(err) = proposal_cache.store(Some(0u16)) {
            error!("Failed to store the current proposal cache: {err}");
        }
    }
}
