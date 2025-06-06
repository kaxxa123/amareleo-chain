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

#![forbid(unsafe_code)]

#[macro_use]
extern crate tracing;

#[macro_use]
extern crate amareleo_chain_tracing;

use amareleo_chain_account::Account;
use amareleo_chain_tracing::{TracingHandler, TracingHandlerGuard};
use amareleo_node_bft::{
    BFT,
    MAX_BATCH_DELAY_IN_MS,
    Primary,
    helpers::{
        ConsensusReceiver,
        PrimaryReceiver,
        PrimarySender,
        Storage as NarwhalStorage,
        fmt_id,
        init_consensus_channels,
    },
    spawn_blocking,
};
use amareleo_node_bft_ledger_service::LedgerService;
use amareleo_node_bft_storage_service::BFTPersistentStorage;
use snarkvm::{
    ledger::{
        block::Transaction,
        narwhal::{BatchHeader, Data, Subdag, Transmission, TransmissionID},
        puzzle::{Solution, SolutionID},
    },
    prelude::*,
};

use aleo_std::StorageMode;
use anyhow::Result;
use colored::Colorize;
use indexmap::IndexMap;
#[cfg(feature = "locktick")]
use locktick::parking_lot::Mutex;
use lru::LruCache;
#[cfg(not(feature = "locktick"))]
use parking_lot::Mutex;
use std::{future::Future, num::NonZeroUsize, sync::Arc, time::Duration};
use tokio::{
    sync::{OnceCell, oneshot},
    task::JoinHandle,
};
use tracing::subscriber::DefaultGuard;

#[cfg(feature = "metrics")]
use std::collections::HashMap;

/// The capacity of the queue reserved for deployments.
/// Note: This is an inbound queue capacity, not a Narwhal-enforced capacity.
const CAPACITY_FOR_DEPLOYMENTS: usize = 1 << 10;
/// The capacity of the queue reserved for executions.
/// Note: This is an inbound queue capacity, not a Narwhal-enforced capacity.
const CAPACITY_FOR_EXECUTIONS: usize = 1 << 10;
/// The capacity of the queue reserved for solutions.
/// Note: This is an inbound queue capacity, not a Narwhal-enforced capacity.
const CAPACITY_FOR_SOLUTIONS: usize = 1 << 10;
/// The **suggested** maximum number of deployments in each interval.
/// Note: This is an inbound queue limit, not a Narwhal-enforced limit.
const MAX_DEPLOYMENTS_PER_INTERVAL: usize = 1;

/// Helper struct to track incoming transactions.
struct TransactionsQueue<N: Network> {
    pub deployments: LruCache<N::TransactionID, Transaction<N>>,
    pub executions: LruCache<N::TransactionID, Transaction<N>>,
}

impl<N: Network> Default for TransactionsQueue<N> {
    fn default() -> Self {
        Self {
            deployments: LruCache::new(NonZeroUsize::new(CAPACITY_FOR_DEPLOYMENTS).unwrap()),
            executions: LruCache::new(NonZeroUsize::new(CAPACITY_FOR_EXECUTIONS).unwrap()),
        }
    }
}

#[derive(Clone)]
pub struct Consensus<N: Network> {
    /// The ledger.
    ledger: Arc<dyn LedgerService<N>>,
    /// The BFT.
    bft: BFT<N>,
    /// The primary sender.
    primary_sender: Arc<OnceCell<PrimarySender<N>>>,
    /// The unconfirmed solutions queue.
    solutions_queue: Arc<Mutex<LruCache<SolutionID<N>, Solution<N>>>>,
    /// The unconfirmed transactions queue.
    transactions_queue: Arc<Mutex<TransactionsQueue<N>>>,
    /// The recently-seen unconfirmed solutions.
    seen_solutions: Arc<Mutex<LruCache<SolutionID<N>, ()>>>,
    /// The recently-seen unconfirmed transactions.
    seen_transactions: Arc<Mutex<LruCache<N::TransactionID, ()>>>,
    #[cfg(feature = "metrics")]
    transmissions_queue_timestamps: Arc<Mutex<HashMap<TransmissionID<N>, i64>>>,
    /// Tracing handle
    tracing: Option<TracingHandler>,
    /// The spawned handles.
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl<N: Network> TracingHandlerGuard for Consensus<N> {
    /// Retruns tracing guard
    fn get_tracing_guard(&self) -> Option<DefaultGuard> {
        self.tracing.as_ref().and_then(|trace_handle| trace_handle.get_tracing_guard())
    }
}

impl<N: Network> Consensus<N> {
    /// Initializes a new instance of consensus.
    pub fn new(
        account: Account<N>,
        ledger: Arc<dyn LedgerService<N>>,
        keep_state: bool,
        storage_mode: StorageMode,
        tracing: Option<TracingHandler>,
    ) -> Result<Self> {
        // Initialize the Narwhal transmissions.
        let transmissions = Arc::new(BFTPersistentStorage::open(storage_mode.clone(), tracing.clone())?);
        // Initialize the Narwhal storage.
        let storage =
            NarwhalStorage::new(ledger.clone(), transmissions, BatchHeader::<N>::MAX_GC_ROUNDS as u64, tracing.clone());
        // Initialize the BFT.
        let bft = BFT::new(account, storage, keep_state, storage_mode, ledger.clone(), tracing.clone())?;
        // Return the consensus.
        Ok(Self {
            ledger,
            bft,
            primary_sender: Default::default(),
            solutions_queue: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(CAPACITY_FOR_SOLUTIONS).unwrap()))),
            transactions_queue: Default::default(),
            seen_solutions: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(1 << 16).unwrap()))),
            seen_transactions: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(1 << 16).unwrap()))),
            #[cfg(feature = "metrics")]
            transmissions_queue_timestamps: Default::default(),
            tracing: tracing.clone(),
            handles: Default::default(),
        })
    }

    /// Run the consensus instance.
    pub async fn run(&mut self, primary_sender: PrimarySender<N>, primary_receiver: PrimaryReceiver<N>) -> Result<()> {
        guard_info!(self, "Starting the consensus instance...");

        // Set the primary sender.
        let result = self.primary_sender.set(primary_sender.clone());
        if result.is_err() {
            bail!("Unexpected: Primary sender already set");
        }

        // Initialize communications between Consensus and BFT.
        let (consensus_sender, consensus_receiver) = init_consensus_channels();

        // Start the Consensus handlers.
        self.start_handlers(consensus_receiver);

        // Run the BFT.
        let result = self.bft.run(Some(consensus_sender), primary_sender, primary_receiver).await;
        if let Err(err) = result {
            guard_error!(self, "Consensus failed to run the BFT instance - {err}");
            self.shut_down().await;
            return Err(err);
        }

        Ok(())
    }

    /// Returns the ledger.
    pub const fn ledger(&self) -> &Arc<dyn LedgerService<N>> {
        &self.ledger
    }

    /// Returns the BFT.
    pub const fn bft(&self) -> &BFT<N> {
        &self.bft
    }

    /// Returns the primary sender.
    fn primary_sender(&self) -> &PrimarySender<N> {
        self.primary_sender.get().expect("Primary sender not set")
    }
}

impl<N: Network> Consensus<N> {
    /// Returns the number of unconfirmed transmissions.
    pub fn num_unconfirmed_transmissions(&self) -> usize {
        self.bft.num_unconfirmed_transmissions()
    }

    /// Returns the number of unconfirmed ratifications.
    pub fn num_unconfirmed_ratifications(&self) -> usize {
        self.bft.num_unconfirmed_ratifications()
    }

    /// Returns the number of solutions.
    pub fn num_unconfirmed_solutions(&self) -> usize {
        self.bft.num_unconfirmed_solutions()
    }

    /// Returns the number of unconfirmed transactions.
    pub fn num_unconfirmed_transactions(&self) -> usize {
        self.bft.num_unconfirmed_transactions()
    }
}

impl<N: Network> Consensus<N> {
    /// Returns the unconfirmed transmission IDs.
    pub fn unconfirmed_transmission_ids(&self) -> impl '_ + Iterator<Item = TransmissionID<N>> {
        self.worker_transmission_ids().chain(self.inbound_transmission_ids())
    }

    /// Returns the unconfirmed transmissions.
    pub fn unconfirmed_transmissions(&self) -> impl '_ + Iterator<Item = (TransmissionID<N>, Transmission<N>)> {
        self.worker_transmissions().chain(self.inbound_transmissions())
    }

    /// Returns the unconfirmed solutions.
    pub fn unconfirmed_solutions(&self) -> impl '_ + Iterator<Item = (SolutionID<N>, Data<Solution<N>>)> {
        self.worker_solutions().chain(self.inbound_solutions())
    }

    /// Returns the unconfirmed transactions.
    pub fn unconfirmed_transactions(&self) -> impl '_ + Iterator<Item = (N::TransactionID, Data<Transaction<N>>)> {
        self.worker_transactions().chain(self.inbound_transactions())
    }
}

impl<N: Network> Consensus<N> {
    /// Returns the worker transmission IDs.
    pub fn worker_transmission_ids(&self) -> impl '_ + Iterator<Item = TransmissionID<N>> {
        self.bft.worker_transmission_ids()
    }

    /// Returns the worker transmissions.
    pub fn worker_transmissions(&self) -> impl '_ + Iterator<Item = (TransmissionID<N>, Transmission<N>)> {
        self.bft.worker_transmissions()
    }

    /// Returns the worker solutions.
    pub fn worker_solutions(&self) -> impl '_ + Iterator<Item = (SolutionID<N>, Data<Solution<N>>)> {
        self.bft.worker_solutions()
    }

    /// Returns the worker transactions.
    pub fn worker_transactions(&self) -> impl '_ + Iterator<Item = (N::TransactionID, Data<Transaction<N>>)> {
        self.bft.worker_transactions()
    }
}

impl<N: Network> Consensus<N> {
    /// Returns the transmission IDs in the inbound queue.
    pub fn inbound_transmission_ids(&self) -> impl '_ + Iterator<Item = TransmissionID<N>> {
        self.inbound_transmissions().map(|(id, _)| id)
    }

    /// Returns the transmissions in the inbound queue.
    pub fn inbound_transmissions(&self) -> impl '_ + Iterator<Item = (TransmissionID<N>, Transmission<N>)> {
        self.inbound_transactions()
            .map(|(id, tx)| {
                (
                    TransmissionID::Transaction(id, tx.to_checksum::<N>().unwrap_or_default()),
                    Transmission::Transaction(tx),
                )
            })
            .chain(self.inbound_solutions().map(|(id, solution)| {
                (
                    TransmissionID::Solution(id, solution.to_checksum::<N>().unwrap_or_default()),
                    Transmission::Solution(solution),
                )
            }))
    }

    /// Returns the solutions in the inbound queue.
    pub fn inbound_solutions(&self) -> impl '_ + Iterator<Item = (SolutionID<N>, Data<Solution<N>>)> {
        // Return an iterator over the solutions in the inbound queue.
        self.solutions_queue.lock().clone().into_iter().map(|(id, solution)| (id, Data::Object(solution)))
    }

    /// Returns the transactions in the inbound queue.
    pub fn inbound_transactions(&self) -> impl '_ + Iterator<Item = (N::TransactionID, Data<Transaction<N>>)> {
        // Acquire the lock on the transactions queue.
        let tx_queue = self.transactions_queue.lock();
        // Return an iterator over the deployment and execution transactions in the inbound queue.
        tx_queue
            .deployments
            .clone()
            .into_iter()
            .chain(tx_queue.executions.clone())
            .map(|(id, tx)| (id, Data::Object(tx)))
    }
}

impl<N: Network> Consensus<N> {
    /// Adds the given unconfirmed solution to the memory pool.
    pub async fn add_unconfirmed_solution(&self, solution: Solution<N>) -> Result<()> {
        // Calculate the transmission checksum.
        let checksum = Data::<Solution<N>>::Buffer(solution.to_bytes_le()?.into()).to_checksum::<N>()?;
        #[cfg(feature = "metrics")]
        {
            metrics::increment_gauge(metrics::consensus::UNCONFIRMED_SOLUTIONS, 1f64);
            let timestamp = amareleo_node_bft::helpers::now();
            self.transmissions_queue_timestamps
                .lock()
                .insert(TransmissionID::Solution(solution.id(), checksum), timestamp);
        }
        // Queue the unconfirmed solution.
        {
            let solution_id = solution.id();

            // Check if the transaction was recently seen.
            if self.seen_solutions.lock().put(solution_id, ()).is_some() {
                // If the transaction was recently seen, return early.
                return Ok(());
            }
            // Check if the solution already exists in the ledger.
            if self.ledger.contains_transmission(&TransmissionID::Solution(solution_id, checksum))? {
                bail!("Solution '{}' exists in the ledger {}", fmt_id(solution_id), "(skipping)".dimmed());
            }
            // Add the solution to the memory pool.
            guard_trace!(self, "Received unconfirmed solution '{}' in the queue", fmt_id(solution_id));
            if self.solutions_queue.lock().put(solution_id, solution).is_some() {
                bail!("Solution '{}' exists in the memory pool", fmt_id(solution_id));
            }
        }

        // Try to process the unconfirmed solutions in the memory pool.
        self.process_unconfirmed_solutions().await
    }

    /// Processes unconfirmed transactions in the memory pool.
    pub async fn process_unconfirmed_solutions(&self) -> Result<()> {
        // If the memory pool of this node is full, return early.
        let num_unconfirmed_solutions = self.num_unconfirmed_solutions();
        let num_unconfirmed_transmissions = self.num_unconfirmed_transmissions();
        if num_unconfirmed_solutions >= N::MAX_SOLUTIONS
            || num_unconfirmed_transmissions >= Primary::<N>::MAX_TRANSMISSIONS_TOLERANCE
        {
            return Ok(());
        }
        // Retrieve the solutions.
        let solutions = {
            // Determine the available capacity.
            let capacity = N::MAX_SOLUTIONS.saturating_sub(num_unconfirmed_solutions);
            // Acquire the lock on the queue.
            let mut queue = self.solutions_queue.lock();
            // Determine the number of solutions to send.
            let num_solutions = queue.len().min(capacity);
            // Drain the solutions from the queue.
            (0..num_solutions).filter_map(|_| queue.pop_lru().map(|(_, solution)| solution)).collect::<Vec<_>>()
        };
        // Iterate over the solutions.
        for solution in solutions.into_iter() {
            let solution_id = solution.id();
            guard_trace!(self, "Adding unconfirmed solution '{}' to the memory pool...", fmt_id(solution_id));
            // Send the unconfirmed solution to the primary.
            if let Err(e) = self.primary_sender().send_unconfirmed_solution(solution_id, Data::Object(solution)).await {
                // If the BFT is synced, then log the warning.
                if self.bft.is_synced() {
                    // If error occurs after the first 10 blocks of the epoch, log it as a warning, otherwise ignore.
                    if self.ledger().latest_block_height() % N::NUM_BLOCKS_PER_EPOCH > 10 {
                        guard_warn!(
                            self,
                            "Failed to add unconfirmed solution '{}' to the memory pool - {e}",
                            fmt_id(solution_id)
                        )
                    };
                }
            }
        }
        Ok(())
    }

    /// Adds the given unconfirmed transaction to the memory pool.
    pub async fn add_unconfirmed_transaction(&self, transaction: Transaction<N>) -> Result<()> {
        // Calculate the transmission checksum.
        let checksum = Data::<Transaction<N>>::Buffer(transaction.to_bytes_le()?.into()).to_checksum::<N>()?;
        #[cfg(feature = "metrics")]
        {
            metrics::increment_gauge(metrics::consensus::UNCONFIRMED_TRANSACTIONS, 1f64);
            let timestamp = amareleo_node_bft::helpers::now();
            self.transmissions_queue_timestamps
                .lock()
                .insert(TransmissionID::Transaction(transaction.id(), checksum), timestamp);
        }
        // Queue the unconfirmed transaction.
        {
            let transaction_id = transaction.id();

            // Check that the transaction is not a fee transaction.
            if transaction.is_fee() {
                bail!("Transaction '{}' is a fee transaction {}", fmt_id(transaction_id), "(skipping)".dimmed());
            }
            // Check if the transaction was recently seen.
            if self.seen_transactions.lock().put(transaction_id, ()).is_some() {
                // If the transaction was recently seen, return early.
                return Ok(());
            }
            // Check if the transaction already exists in the ledger.
            if self.ledger.contains_transmission(&TransmissionID::Transaction(transaction_id, checksum))? {
                bail!("Transaction '{}' exists in the ledger {}", fmt_id(transaction_id), "(skipping)".dimmed());
            }
            // Add the transaction to the memory pool.
            guard_trace!(self, "Received unconfirmed transaction '{}' in the queue", fmt_id(transaction_id));
            if transaction.is_deploy() {
                if self.transactions_queue.lock().deployments.put(transaction_id, transaction).is_some() {
                    bail!("Transaction '{}' exists in the memory pool", fmt_id(transaction_id));
                }
            } else if self.transactions_queue.lock().executions.put(transaction_id, transaction).is_some() {
                bail!("Transaction '{}' exists in the memory pool", fmt_id(transaction_id));
            }

            // Try to process the unconfirmed transactions in the memory pool.
            self.process_unconfirmed_transactions().await
        }
    }

    /// Processes unconfirmed transactions in the memory pool.
    pub async fn process_unconfirmed_transactions(&self) -> Result<()> {
        // If the memory pool of this node is full, return early.
        let num_unconfirmed_transmissions = self.num_unconfirmed_transmissions();
        if num_unconfirmed_transmissions >= Primary::<N>::MAX_TRANSMISSIONS_TOLERANCE {
            return Ok(());
        }
        // Retrieve the transactions.
        let transactions = {
            // Determine the available capacity.
            let capacity = Primary::<N>::MAX_TRANSMISSIONS_TOLERANCE.saturating_sub(num_unconfirmed_transmissions);
            // Acquire the lock on the transactions queue.
            let mut tx_queue = self.transactions_queue.lock();
            // Determine the number of deployments to send.
            let num_deployments = tx_queue.deployments.len().min(capacity).min(MAX_DEPLOYMENTS_PER_INTERVAL);
            // Determine the number of executions to send.
            let num_executions = tx_queue.executions.len().min(capacity.saturating_sub(num_deployments));
            // Create an iterator which will select interleaved deployments and executions within the capacity.
            // Note: interleaving ensures we will never have consecutive invalid deployments blocking the queue.
            let selector_iter = (0..num_deployments).map(|_| true).interleave((0..num_executions).map(|_| false));
            // Drain the transactions from the queue, interleaving deployments and executions.
            selector_iter
                .filter_map(|select_deployment| {
                    if select_deployment {
                        tx_queue.deployments.pop_lru().map(|(_, tx)| tx)
                    } else {
                        tx_queue.executions.pop_lru().map(|(_, tx)| tx)
                    }
                })
                .collect_vec()
        };
        // Iterate over the transactions.
        for transaction in transactions.into_iter() {
            let transaction_id = transaction.id();
            guard_trace!(self, "Adding unconfirmed transaction '{}' to the memory pool...", fmt_id(transaction_id));
            // Send the unconfirmed transaction to the primary.
            if let Err(e) =
                self.primary_sender().send_unconfirmed_transaction(transaction_id, Data::Object(transaction)).await
            {
                // If the BFT is synced, then log the warning.
                if self.bft.is_synced() {
                    guard_warn!(
                        self,
                        "Failed to add unconfirmed transaction '{}' to the memory pool - {e}",
                        fmt_id(transaction_id)
                    );
                }
            }
        }
        Ok(())
    }
}

impl<N: Network> Consensus<N> {
    /// Starts the consensus handlers.
    fn start_handlers(&self, consensus_receiver: ConsensusReceiver<N>) {
        let ConsensusReceiver { mut rx_consensus_subdag } = consensus_receiver;

        // Process the committed subdag and transmissions from the BFT.
        let self_ = self.clone();
        self.spawn(async move {
            while let Some((committed_subdag, transmissions, callback)) = rx_consensus_subdag.recv().await {
                self_.process_bft_subdag(committed_subdag, transmissions, callback).await;
            }
        });

        // Process the unconfirmed transactions in the memory pool.
        let self_ = self.clone();
        self.spawn(async move {
            loop {
                // Sleep briefly.
                tokio::time::sleep(Duration::from_millis(MAX_BATCH_DELAY_IN_MS)).await;
                // Process the unconfirmed transactions in the memory pool.
                if let Err(e) = self_.process_unconfirmed_transactions().await {
                    guard_warn!(self_, "Cannot process unconfirmed transactions - {e}");
                }
                // Process the unconfirmed solutions in the memory pool.
                if let Err(e) = self_.process_unconfirmed_solutions().await {
                    guard_warn!(self_, "Cannot process unconfirmed solutions - {e}");
                }
            }
        });
    }

    /// Processes the committed subdag and transmissions from the BFT.
    async fn process_bft_subdag(
        &self,
        subdag: Subdag<N>,
        transmissions: IndexMap<TransmissionID<N>, Transmission<N>>,
        callback: oneshot::Sender<Result<()>>,
    ) {
        // Try to advance to the next block.
        let self_ = self.clone();
        let transmissions_ = transmissions.clone();
        let result = spawn_blocking! { self_.try_advance_to_next_block(subdag, transmissions_) };

        // If the block failed to advance, reinsert the transmissions into the memory pool.
        if let Err(e) = &result {
            guard_error!(self, "Unable to advance to the next block - {e}");
            // On failure, reinsert the transmissions into the memory pool.
            self.reinsert_transmissions(transmissions).await;
        }
        // Send the callback **after** advancing to the next block.
        // Note: We must await the block to be advanced before sending the callback.
        callback.send(result).ok();
    }

    /// Attempts to advance to the next block.
    fn try_advance_to_next_block(
        &self,
        subdag: Subdag<N>,
        transmissions: IndexMap<TransmissionID<N>, Transmission<N>>,
    ) -> Result<()> {
        #[cfg(feature = "metrics")]
        let start = subdag.leader_certificate().batch_header().timestamp();
        #[cfg(feature = "metrics")]
        let num_committed_certificates = subdag.values().map(|c| c.len()).sum::<usize>();
        #[cfg(feature = "metrics")]
        let current_block_timestamp = self.ledger.latest_block().header().metadata().timestamp();

        // Create the candidate next block.
        let next_block = self.ledger.prepare_advance_to_next_quorum_block(subdag, transmissions)?;

        // AlexZ: Disabling block validation since this would catch our
        //        cheating in always assigning the same leader.
        //        Validation is within SnarkVM which we don't want to touch.
        // Check that the block is well-formed.
        // self.ledger.check_next_block(&next_block)?;

        // Advance to the next block.
        self.ledger.advance_to_next_block(&next_block)?;

        // If the next block starts a new epoch, clear the existing solutions.
        if next_block.height() % N::NUM_BLOCKS_PER_EPOCH == 0 {
            // Clear the solutions queue.
            self.solutions_queue.lock().clear();
            // Clear the worker solutions.
            self.bft.primary().clear_worker_solutions();
        }

        #[cfg(feature = "metrics")]
        {
            let elapsed = std::time::Duration::from_secs((amareleo_node_bft::helpers::now() - start) as u64);
            let next_block_timestamp = next_block.header().metadata().timestamp();
            let block_latency = next_block_timestamp - current_block_timestamp;
            let proof_target = next_block.header().proof_target();
            let coinbase_target = next_block.header().coinbase_target();
            let cumulative_proof_target = next_block.header().cumulative_proof_target();

            metrics::add_transmission_latency_metric(&self.transmissions_queue_timestamps, &next_block);

            metrics::gauge(metrics::consensus::COMMITTED_CERTIFICATES, num_committed_certificates as f64);
            metrics::histogram(metrics::consensus::CERTIFICATE_COMMIT_LATENCY, elapsed.as_secs_f64());
            metrics::histogram(metrics::consensus::BLOCK_LATENCY, block_latency as f64);
            metrics::gauge(metrics::blocks::PROOF_TARGET, proof_target as f64);
            metrics::gauge(metrics::blocks::COINBASE_TARGET, coinbase_target as f64);
            metrics::gauge(metrics::blocks::CUMULATIVE_PROOF_TARGET, cumulative_proof_target as f64);
        }
        Ok(())
    }

    /// Reinserts the given transmissions into the memory pool.
    async fn reinsert_transmissions(&self, transmissions: IndexMap<TransmissionID<N>, Transmission<N>>) {
        // Iterate over the transmissions.
        for (transmission_id, transmission) in transmissions.into_iter() {
            // Reinsert the transmission into the memory pool.
            if let Err(e) = self.reinsert_transmission(transmission_id, transmission).await {
                guard_warn!(
                    self,
                    "Unable to reinsert transmission {}.{} into the memory pool - {e}",
                    fmt_id(transmission_id),
                    fmt_id(transmission_id.checksum().unwrap_or_default()).dimmed()
                );
            }
        }
    }

    /// Reinserts the given transmission into the memory pool.
    async fn reinsert_transmission(
        &self,
        transmission_id: TransmissionID<N>,
        transmission: Transmission<N>,
    ) -> Result<()> {
        // Initialize a callback sender and receiver.
        let (callback, callback_receiver) = oneshot::channel();
        // Send the transmission to the primary.
        match (transmission_id, transmission) {
            (TransmissionID::Ratification, Transmission::Ratification) => return Ok(()),
            (TransmissionID::Solution(solution_id, _), Transmission::Solution(solution)) => {
                // Send the solution to the primary.
                self.primary_sender().tx_unconfirmed_solution.send((solution_id, solution, callback)).await?;
            }
            (TransmissionID::Transaction(transaction_id, _), Transmission::Transaction(transaction)) => {
                // Send the transaction to the primary.
                self.primary_sender().tx_unconfirmed_transaction.send((transaction_id, transaction, callback)).await?;
            }
            _ => bail!("Mismatching `(transmission_id, transmission)` pair in consensus"),
        }
        // Await the callback.
        callback_receiver.await?
    }

    /// Spawns a task with the given future; it should only be used for long-running tasks.
    fn spawn<T: Future<Output = ()> + Send + 'static>(&self, future: T) {
        self.handles.lock().push(tokio::spawn(future));
    }

    /// Shuts down the BFT.
    pub async fn shut_down(&self) {
        guard_info!(self, "Shutting down consensus...");

        // Shut down the BFT.
        self.bft.shut_down().await;

        // Abort Consensus tasks.
        let mut handles = self.handles.lock();
        handles.iter().for_each(|handle| handle.abort());
        handles.clear();
    }
}
