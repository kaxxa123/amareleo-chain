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
    helpers::{fmt_id, Ready, Storage},
    ProposedBatch, MAX_WORKERS,
};
use snarkos_lite_node_bft_ledger_service::LedgerService;
use snarkvm::{
    console::prelude::*,
    ledger::{
        block::Transaction,
        narwhal::{BatchHeader, Data, Transmission, TransmissionID},
        puzzle::{Solution, SolutionID},
    },
};

use colored::Colorize;
use indexmap::{IndexMap, IndexSet};
use std::sync::Arc;

#[derive(Clone)]
pub struct Worker<N: Network> {
    /// The worker ID.
    id: u8,
    /// The storage.
    storage: Storage<N>,
    /// The ledger service.
    ledger: Arc<dyn LedgerService<N>>,
    /// The proposed batch.
    proposed_batch: Arc<ProposedBatch<N>>,
    /// The ready queue.
    ready: Ready<N>,
}

impl<N: Network> Worker<N> {
    /// Initializes a new worker instance.
    pub fn new(
        id: u8,
        storage: Storage<N>,
        ledger: Arc<dyn LedgerService<N>>,
        proposed_batch: Arc<ProposedBatch<N>>,
    ) -> Result<Self> {
        // Ensure the worker ID is valid.
        ensure!(id < MAX_WORKERS, "Invalid worker ID '{id}'");
        // Return the worker.
        Ok(Self {
            id,
            storage,
            ledger,
            proposed_batch,
            ready: Default::default(),
        })
    }

    /// Returns the worker ID.
    pub const fn id(&self) -> u8 {
        self.id
    }
}

impl<N: Network> Worker<N> {
    /// The maximum number of transmissions allowed in a worker.
    pub const MAX_TRANSMISSIONS_PER_WORKER: usize =
        BatchHeader::<N>::MAX_TRANSMISSIONS_PER_BATCH / MAX_WORKERS as usize;
    /// The maximum number of transmissions allowed in a worker ping.
    pub const MAX_TRANSMISSIONS_PER_WORKER_PING: usize =
        BatchHeader::<N>::MAX_TRANSMISSIONS_PER_BATCH / 10;

    // transmissions

    /// Returns the number of transmissions in the ready queue.
    pub fn num_transmissions(&self) -> usize {
        self.ready.num_transmissions()
    }

    /// Returns the number of ratifications in the ready queue.
    pub fn num_ratifications(&self) -> usize {
        self.ready.num_ratifications()
    }

    /// Returns the number of solutions in the ready queue.
    pub fn num_solutions(&self) -> usize {
        self.ready.num_solutions()
    }

    /// Returns the number of transactions in the ready queue.
    pub fn num_transactions(&self) -> usize {
        self.ready.num_transactions()
    }
}

impl<N: Network> Worker<N> {
    /// Returns the transmission IDs in the ready queue.
    pub fn transmission_ids(&self) -> IndexSet<TransmissionID<N>> {
        self.ready.transmission_ids()
    }

    /// Returns the transmissions in the ready queue.
    pub fn transmissions(&self) -> IndexMap<TransmissionID<N>, Transmission<N>> {
        self.ready.transmissions()
    }

    /// Returns the solutions in the ready queue.
    pub fn solutions(&self) -> impl '_ + Iterator<Item = (SolutionID<N>, Data<Solution<N>>)> {
        self.ready.solutions()
    }

    /// Returns the transactions in the ready queue.
    pub fn transactions(
        &self,
    ) -> impl '_ + Iterator<Item = (N::TransactionID, Data<Transaction<N>>)> {
        self.ready.transactions()
    }
}

impl<N: Network> Worker<N> {
    /// Clears the solutions from the ready queue.
    pub(super) fn clear_solutions(&self) {
        self.ready.clear_solutions()
    }
}

impl<N: Network> Worker<N> {
    /// Returns `true` if the transmission ID exists in the ready queue, proposed batch, storage, or ledger.
    pub fn contains_transmission(&self, transmission_id: impl Into<TransmissionID<N>>) -> bool {
        let transmission_id = transmission_id.into();
        // Check if the transmission ID exists in the ready queue, proposed batch, storage, or ledger.
        self.ready.contains(transmission_id)
            || self
                .proposed_batch
                .read()
                .as_ref()
                .map_or(false, |p| p.contains_transmission(transmission_id))
            || self.storage.contains_transmission(transmission_id)
            || self
                .ledger
                .contains_transmission(&transmission_id)
                .unwrap_or(false)
    }

    /// Returns the transmission if it exists in the ready queue, proposed batch, storage.
    ///
    /// Note: We explicitly forbid retrieving a transmission from the ledger, as transmissions
    /// in the ledger are not guaranteed to be invalid for the current batch.
    pub fn get_transmission(&self, transmission_id: TransmissionID<N>) -> Option<Transmission<N>> {
        // Check if the transmission ID exists in the ready queue.
        if let Some(transmission) = self.ready.get(transmission_id) {
            return Some(transmission);
        }
        // Check if the transmission ID exists in storage.
        if let Some(transmission) = self.storage.get_transmission(transmission_id) {
            return Some(transmission);
        }
        // Check if the transmission ID exists in the proposed batch.
        if let Some(transmission) = self
            .proposed_batch
            .read()
            .as_ref()
            .and_then(|p| p.get_transmission(transmission_id))
        {
            return Some(transmission.clone());
        }
        None
    }

    /// Returns the transmissions if it exists in the worker, or requests it from the specified peer.
    pub async fn get_or_fetch_transmission(
        &self,
        transmission_id: TransmissionID<N>,
    ) -> Result<(TransmissionID<N>, Transmission<N>)> {
        // Attempt to get the transmission from the worker.
        if let Some(transmission) = self.get_transmission(transmission_id) {
            return Ok((transmission_id, transmission));
        }

        bail!("Unable to fetch transmission");
    }

    /// Removes up to the specified number of transmissions from the ready queue, and returns them.
    pub(crate) fn drain(
        &self,
        num_transmissions: usize,
    ) -> impl Iterator<Item = (TransmissionID<N>, Transmission<N>)> {
        self.ready.drain(num_transmissions).into_iter()
    }

    /// Reinserts the specified transmission into the ready queue.
    pub(crate) fn reinsert(
        &self,
        transmission_id: TransmissionID<N>,
        transmission: Transmission<N>,
    ) -> bool {
        // Check if the transmission ID exists.
        if !self.contains_transmission(transmission_id) {
            // Insert the transmission into the ready queue.
            return self.ready.insert(transmission_id, transmission);
        }
        false
    }
}

impl<N: Network> Worker<N> {
    /// Handles the incoming unconfirmed solution.
    /// Note: This method assumes the incoming solution is valid and does not exist in the ledger.
    pub(crate) async fn process_unconfirmed_solution(
        &self,
        solution_id: SolutionID<N>,
        solution: Data<Solution<N>>,
    ) -> Result<()> {
        // Construct the transmission.
        let transmission = Transmission::Solution(solution.clone());
        // Compute the checksum.
        let checksum = solution.to_checksum::<N>()?;
        // Construct the transmission ID.
        let transmission_id = TransmissionID::Solution(solution_id, checksum);
        // Check if the solution exists.
        if self.contains_transmission(transmission_id) {
            bail!(
                "Solution '{}.{}' already exists.",
                fmt_id(solution_id),
                fmt_id(checksum).dimmed()
            );
        }
        // Check that the solution is well-formed and unique.
        self.ledger
            .check_solution_basic(solution_id, solution)
            .await?;
        // Adds the solution to the ready queue.
        if self.ready.insert(transmission_id, transmission) {
            trace!(
                "Worker {} - Added unconfirmed solution '{}.{}'",
                self.id,
                fmt_id(solution_id),
                fmt_id(checksum).dimmed()
            );
        }
        Ok(())
    }

    /// Handles the incoming unconfirmed transaction.
    pub(crate) async fn process_unconfirmed_transaction(
        &self,
        transaction_id: N::TransactionID,
        transaction: Data<Transaction<N>>,
    ) -> Result<()> {
        // Construct the transmission.
        let transmission = Transmission::Transaction(transaction.clone());
        // Compute the checksum.
        let checksum = transaction.to_checksum::<N>()?;
        // Construct the transmission ID.
        let transmission_id = TransmissionID::Transaction(transaction_id, checksum);
        // Check if the transaction ID exists.
        if self.contains_transmission(transmission_id) {
            bail!(
                "Transaction '{}.{}' already exists.",
                fmt_id(transaction_id),
                fmt_id(checksum).dimmed()
            );
        }
        // Check that the transaction is well-formed and unique.
        self.ledger
            .check_transaction_basic(transaction_id, transaction)
            .await?;
        // Adds the transaction to the ready queue.
        if self.ready.insert(transmission_id, transmission) {
            trace!(
                "Worker {}.{} - Added unconfirmed transaction '{}'",
                self.id,
                fmt_id(transaction_id),
                fmt_id(checksum).dimmed()
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use snarkos_lite_node_bft_ledger_service::LedgerService;
    use snarkos_lite_node_bft_storage_service::BFTMemoryService;
    use snarkvm::{
        console::{network::Network, types::Field},
        ledger::{
            block::Block,
            committee::Committee,
            narwhal::{BatchCertificate, Subdag, Transmission, TransmissionID},
        },
        prelude::Address,
    };

    use bytes::Bytes;
    use indexmap::IndexMap;
    use mockall::mock;
    use std::ops::Range;

    type CurrentNetwork = snarkvm::prelude::MainnetV0;

    const ITERATIONS: usize = 100;

    mock! {
        #[derive(Debug)]
        Ledger<N: Network> {}
        #[async_trait]
        impl<N: Network> LedgerService<N> for Ledger<N> {
            fn latest_round(&self) -> u64;
            fn latest_block_height(&self) -> u32;
            fn latest_block(&self) -> Block<N>;
            fn latest_restrictions_id(&self) -> Field<N>;
            fn latest_leader(&self) -> Option<(u64, Address<N>)>;
            fn update_latest_leader(&self, round: u64, leader: Address<N>);
            fn contains_block_height(&self, height: u32) -> bool;
            fn get_block_height(&self, hash: &N::BlockHash) -> Result<u32>;
            fn get_block_hash(&self, height: u32) -> Result<N::BlockHash>;
            fn get_block_round(&self, height: u32) -> Result<u64>;
            fn get_block(&self, height: u32) -> Result<Block<N>>;
            fn get_blocks(&self, heights: Range<u32>) -> Result<Vec<Block<N>>>;
            fn get_solution(&self, solution_id: &SolutionID<N>) -> Result<Solution<N>>;
            fn get_unconfirmed_transaction(&self, transaction_id: N::TransactionID) -> Result<Transaction<N>>;
            fn get_batch_certificate(&self, certificate_id: &Field<N>) -> Result<BatchCertificate<N>>;
            fn current_committee(&self) -> Result<Committee<N>>;
            fn get_committee_for_round(&self, round: u64) -> Result<Committee<N>>;
            fn get_committee_lookback_for_round(&self, round: u64) -> Result<Committee<N>>;
            fn contains_certificate(&self, certificate_id: &Field<N>) -> Result<bool>;
            fn contains_transmission(&self, transmission_id: &TransmissionID<N>) -> Result<bool>;
            fn ensure_transmission_is_well_formed(
                &self,
                transmission_id: TransmissionID<N>,
                transmission: &mut Transmission<N>,
            ) -> Result<()>;
            async fn check_solution_basic(
                &self,
                solution_id: SolutionID<N>,
                solution: Data<Solution<N>>,
            ) -> Result<()>;
            async fn check_transaction_basic(
                &self,
                transaction_id: N::TransactionID,
                transaction: Data<Transaction<N>>,
            ) -> Result<()>;
            fn check_next_block(&self, block: &Block<N>) -> Result<()>;
            fn prepare_advance_to_next_quorum_block(
                &self,
                subdag: Subdag<N>,
                transmissions: IndexMap<TransmissionID<N>, Transmission<N>>,
            ) -> Result<Block<N>>;
            fn advance_to_next_block(&self, block: &Block<N>) -> Result<()>;
        }
    }

    #[tokio::test]
    async fn test_process_solution_ok() {
        let rng = &mut TestRng::default();
        // Sample a committee.
        let committee = snarkvm::ledger::committee::test_helpers::sample_committee(rng);
        let committee_clone = committee.clone();

        let mut mock_ledger = MockLedger::default();
        mock_ledger
            .expect_current_committee()
            .returning(move || Ok(committee.clone()));
        mock_ledger
            .expect_get_committee_lookback_for_round()
            .returning(move |_| Ok(committee_clone.clone()));
        mock_ledger
            .expect_contains_transmission()
            .returning(|_| Ok(false));
        mock_ledger
            .expect_check_solution_basic()
            .returning(|_, _| Ok(()));
        let ledger: Arc<dyn LedgerService<CurrentNetwork>> = Arc::new(mock_ledger);
        // Initialize the storage.
        let storage =
            Storage::<CurrentNetwork>::new(ledger.clone(), Arc::new(BFTMemoryService::new()), 1);

        // Create the Worker.
        let worker = Worker::new(0, storage, ledger, Default::default()).unwrap();
        let solution = Data::Buffer(Bytes::from(
            (0..512).map(|_| rng.gen::<u8>()).collect::<Vec<_>>(),
        ));
        let solution_id = rng.gen::<u64>().into();
        let solution_checksum = solution.to_checksum::<CurrentNetwork>().unwrap();
        let transmission_id = TransmissionID::Solution(solution_id, solution_checksum);
        let result = worker
            .process_unconfirmed_solution(solution_id, solution)
            .await;
        assert!(result.is_ok());
        assert!(worker.ready.contains(transmission_id));
    }

    #[tokio::test]
    async fn test_process_solution_nok() {
        let rng = &mut TestRng::default();
        // Sample a committee.
        let committee = snarkvm::ledger::committee::test_helpers::sample_committee(rng);
        let committee_clone = committee.clone();

        let mut mock_ledger = MockLedger::default();
        mock_ledger
            .expect_current_committee()
            .returning(move || Ok(committee.clone()));
        mock_ledger
            .expect_get_committee_lookback_for_round()
            .returning(move |_| Ok(committee_clone.clone()));
        mock_ledger
            .expect_contains_transmission()
            .returning(|_| Ok(false));
        mock_ledger
            .expect_check_solution_basic()
            .returning(|_, _| Err(anyhow!("")));
        let ledger: Arc<dyn LedgerService<CurrentNetwork>> = Arc::new(mock_ledger);
        // Initialize the storage.
        let storage =
            Storage::<CurrentNetwork>::new(ledger.clone(), Arc::new(BFTMemoryService::new()), 1);

        // Create the Worker.
        let worker = Worker::new(0, storage, ledger, Default::default()).unwrap();
        let solution_id = rng.gen::<u64>().into();
        let solution = Data::Buffer(Bytes::from(
            (0..512).map(|_| rng.gen::<u8>()).collect::<Vec<_>>(),
        ));
        let checksum = solution.to_checksum::<CurrentNetwork>().unwrap();
        let transmission_id = TransmissionID::Solution(solution_id, checksum);
        let result = worker
            .process_unconfirmed_solution(solution_id, solution)
            .await;
        assert!(result.is_err());
        assert!(!worker.ready.contains(transmission_id));
    }

    #[tokio::test]
    async fn test_process_transaction_ok() {
        let mut rng = &mut TestRng::default();
        // Sample a committee.
        let committee = snarkvm::ledger::committee::test_helpers::sample_committee(rng);
        let committee_clone = committee.clone();

        let mut mock_ledger = MockLedger::default();
        mock_ledger
            .expect_current_committee()
            .returning(move || Ok(committee.clone()));
        mock_ledger
            .expect_get_committee_lookback_for_round()
            .returning(move |_| Ok(committee_clone.clone()));
        mock_ledger
            .expect_contains_transmission()
            .returning(|_| Ok(false));
        mock_ledger
            .expect_check_transaction_basic()
            .returning(|_, _| Ok(()));
        let ledger: Arc<dyn LedgerService<CurrentNetwork>> = Arc::new(mock_ledger);
        // Initialize the storage.
        let storage =
            Storage::<CurrentNetwork>::new(ledger.clone(), Arc::new(BFTMemoryService::new()), 1);

        // Create the Worker.
        let worker = Worker::new(0, storage, ledger, Default::default()).unwrap();
        let transaction_id: <CurrentNetwork as Network>::TransactionID =
            Field::<CurrentNetwork>::rand(&mut rng).into();
        let transaction = Data::Buffer(Bytes::from(
            (0..512).map(|_| rng.gen::<u8>()).collect::<Vec<_>>(),
        ));
        let checksum = transaction.to_checksum::<CurrentNetwork>().unwrap();
        let transmission_id = TransmissionID::Transaction(transaction_id, checksum);
        let result = worker
            .process_unconfirmed_transaction(transaction_id, transaction)
            .await;
        assert!(result.is_ok());
        assert!(worker.ready.contains(transmission_id));
    }

    #[tokio::test]
    async fn test_process_transaction_nok() {
        let mut rng = &mut TestRng::default();
        // Sample a committee.
        let committee = snarkvm::ledger::committee::test_helpers::sample_committee(rng);
        let committee_clone = committee.clone();

        let mut mock_ledger = MockLedger::default();
        mock_ledger
            .expect_current_committee()
            .returning(move || Ok(committee.clone()));
        mock_ledger
            .expect_get_committee_lookback_for_round()
            .returning(move |_| Ok(committee_clone.clone()));
        mock_ledger
            .expect_contains_transmission()
            .returning(|_| Ok(false));
        mock_ledger
            .expect_check_transaction_basic()
            .returning(|_, _| Err(anyhow!("")));
        let ledger: Arc<dyn LedgerService<CurrentNetwork>> = Arc::new(mock_ledger);
        // Initialize the storage.
        let storage =
            Storage::<CurrentNetwork>::new(ledger.clone(), Arc::new(BFTMemoryService::new()), 1);

        // Create the Worker.
        let worker = Worker::new(0, storage, ledger, Default::default()).unwrap();
        let transaction_id: <CurrentNetwork as Network>::TransactionID =
            Field::<CurrentNetwork>::rand(&mut rng).into();
        let transaction = Data::Buffer(Bytes::from(
            (0..512).map(|_| rng.gen::<u8>()).collect::<Vec<_>>(),
        ));
        let checksum = transaction.to_checksum::<CurrentNetwork>().unwrap();
        let transmission_id = TransmissionID::Transaction(transaction_id, checksum);
        let result = worker
            .process_unconfirmed_transaction(transaction_id, transaction)
            .await;
        assert!(result.is_err());
        assert!(!worker.ready.contains(transmission_id));
    }

    #[tokio::test]
    async fn test_storage_gc_on_initialization() {
        let rng = &mut TestRng::default();

        for _ in 0..ITERATIONS {
            // Mock the ledger round.
            let max_gc_rounds = rng.gen_range(50..=100);
            let latest_ledger_round = rng.gen_range((max_gc_rounds + 1)..1000);
            let expected_gc_round = latest_ledger_round - max_gc_rounds;

            // Sample a committee.
            let committee = snarkvm::ledger::committee::test_helpers::sample_committee_for_round(
                latest_ledger_round,
                rng,
            );

            let mut mock_ledger = MockLedger::default();
            mock_ledger
                .expect_current_committee()
                .returning(move || Ok(committee.clone()));

            let ledger: Arc<dyn LedgerService<CurrentNetwork>> = Arc::new(mock_ledger);
            // Initialize the storage.
            let storage = Storage::<CurrentNetwork>::new(
                ledger.clone(),
                Arc::new(BFTMemoryService::new()),
                max_gc_rounds,
            );

            // Ensure that the storage GC round is correct.
            assert_eq!(storage.gc_round(), expected_gc_round);
        }
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use snarkos_lite_node_bft_ledger_service::MockLedgerService;
    use snarkvm::{
        console::account::Address,
        ledger::committee::{Committee, MIN_VALIDATOR_STAKE},
    };

    use test_strategy::proptest;

    type CurrentNetwork = snarkvm::prelude::MainnetV0;

    // Initializes a new test committee.
    fn new_test_committee(n: u16) -> Committee<CurrentNetwork> {
        let mut members = IndexMap::with_capacity(n as usize);
        for i in 0..n {
            // Sample the address.
            let rng = &mut TestRng::fixed(i as u64);
            let address = Address::new(rng.gen());
            info!("Validator {i}: {address}");
            members.insert(address, (MIN_VALIDATOR_STAKE, false, rng.gen_range(0..100)));
        }
        // Initialize the committee.
        Committee::<CurrentNetwork>::new(1u64, members).unwrap()
    }

    #[proptest]
    fn worker_initialization(#[strategy(0..MAX_WORKERS)] id: u8, storage: Storage<CurrentNetwork>) {
        let committee = new_test_committee(4);
        let ledger: Arc<dyn LedgerService<CurrentNetwork>> =
            Arc::new(MockLedgerService::new(committee));
        let worker = Worker::new(id, storage, ledger, Default::default()).unwrap();
        assert_eq!(worker.id(), id);
    }

    #[proptest]
    fn invalid_worker_id(#[strategy(MAX_WORKERS..)] id: u8, storage: Storage<CurrentNetwork>) {
        let committee = new_test_committee(4);
        let ledger: Arc<dyn LedgerService<CurrentNetwork>> =
            Arc::new(MockLedgerService::new(committee));
        let worker = Worker::new(id, storage, ledger, Default::default());
        // TODO once Worker implements Debug, simplify this with `unwrap_err`
        if let Err(error) = worker {
            assert_eq!(error.to_string(), format!("Invalid worker ID '{}'", id));
        }
    }
}
