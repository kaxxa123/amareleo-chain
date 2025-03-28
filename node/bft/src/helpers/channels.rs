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

use snarkvm::{
    console::network::*,
    ledger::{
        block::Transaction,
        narwhal::{BatchCertificate, Data, Subdag, Transmission, TransmissionID},
        puzzle::{Solution, SolutionID},
    },
    prelude::Result,
};

use indexmap::IndexMap;
use tokio::sync::{mpsc, oneshot};

const MAX_CHANNEL_SIZE: usize = 8192;

#[derive(Debug)]
pub struct ConsensusSender<N: Network> {
    pub tx_consensus_subdag:
        mpsc::Sender<(Subdag<N>, IndexMap<TransmissionID<N>, Transmission<N>>, oneshot::Sender<Result<()>>)>,
}

#[derive(Debug)]
pub struct ConsensusReceiver<N: Network> {
    pub rx_consensus_subdag:
        mpsc::Receiver<(Subdag<N>, IndexMap<TransmissionID<N>, Transmission<N>>, oneshot::Sender<Result<()>>)>,
}

/// Initializes the consensus channels.
pub fn init_consensus_channels<N: Network>() -> (ConsensusSender<N>, ConsensusReceiver<N>) {
    let (tx_consensus_subdag, rx_consensus_subdag) = mpsc::channel(MAX_CHANNEL_SIZE);

    let sender = ConsensusSender { tx_consensus_subdag };
    let receiver = ConsensusReceiver { rx_consensus_subdag };

    (sender, receiver)
}

#[derive(Clone, Debug)]
pub struct BFTSender<N: Network> {
    pub tx_primary_round: mpsc::Sender<(u64, oneshot::Sender<bool>)>,
    pub tx_primary_certificate: mpsc::Sender<(BatchCertificate<N>, oneshot::Sender<Result<()>>)>,
    pub tx_sync_bft_dag_at_bootup: mpsc::Sender<Vec<BatchCertificate<N>>>,
}

impl<N: Network> BFTSender<N> {
    /// Sends the current round to the BFT.
    pub async fn send_primary_round_to_bft(&self, current_round: u64) -> Result<bool> {
        // Initialize a callback sender and receiver.
        let (callback_sender, callback_receiver) = oneshot::channel();
        // Send the current round to the BFT.
        self.tx_primary_round.send((current_round, callback_sender)).await?;
        // Await the callback to continue.
        Ok(callback_receiver.await?)
    }

    /// Sends the batch certificate to the BFT.
    pub async fn send_primary_certificate_to_bft(&self, certificate: BatchCertificate<N>) -> Result<()> {
        // Initialize a callback sender and receiver.
        let (callback_sender, callback_receiver) = oneshot::channel();
        // Send the certificate to the BFT.
        self.tx_primary_certificate.send((certificate, callback_sender)).await?;
        // Await the callback to continue.
        callback_receiver.await?
    }
}

#[derive(Debug)]
pub struct BFTReceiver<N: Network> {
    pub rx_primary_round: mpsc::Receiver<(u64, oneshot::Sender<bool>)>,
    pub rx_primary_certificate: mpsc::Receiver<(BatchCertificate<N>, oneshot::Sender<Result<()>>)>,
    pub rx_sync_bft_dag_at_bootup: mpsc::Receiver<Vec<BatchCertificate<N>>>,
}

/// Initializes the BFT channels.
pub fn init_bft_channels<N: Network>() -> (BFTSender<N>, BFTReceiver<N>) {
    let (tx_primary_round, rx_primary_round) = mpsc::channel(MAX_CHANNEL_SIZE);
    let (tx_primary_certificate, rx_primary_certificate) = mpsc::channel(MAX_CHANNEL_SIZE);
    let (tx_sync_bft_dag_at_bootup, rx_sync_bft_dag_at_bootup) = mpsc::channel(MAX_CHANNEL_SIZE);

    let sender = BFTSender { tx_primary_round, tx_primary_certificate, tx_sync_bft_dag_at_bootup };
    let receiver = BFTReceiver { rx_primary_round, rx_primary_certificate, rx_sync_bft_dag_at_bootup };

    (sender, receiver)
}

#[derive(Clone, Debug)]
pub struct PrimarySender<N: Network> {
    pub tx_unconfirmed_solution: mpsc::Sender<(SolutionID<N>, Data<Solution<N>>, oneshot::Sender<Result<()>>)>,
    pub tx_unconfirmed_transaction: mpsc::Sender<(N::TransactionID, Data<Transaction<N>>, oneshot::Sender<Result<()>>)>,
}

impl<N: Network> PrimarySender<N> {
    /// Sends the unconfirmed solution to the primary.
    pub async fn send_unconfirmed_solution(
        &self,
        solution_id: SolutionID<N>,
        solution: Data<Solution<N>>,
    ) -> Result<()> {
        // Initialize a callback sender and receiver.
        let (callback_sender, callback_receiver) = oneshot::channel();
        // Send the unconfirmed solution to the primary.
        self.tx_unconfirmed_solution.send((solution_id, solution, callback_sender)).await?;
        // Await the callback to continue.
        callback_receiver.await?
    }

    /// Sends the unconfirmed transaction to the primary.
    pub async fn send_unconfirmed_transaction(
        &self,
        transaction_id: N::TransactionID,
        transaction: Data<Transaction<N>>,
    ) -> Result<()> {
        // Initialize a callback sender and receiver.
        let (callback_sender, callback_receiver) = oneshot::channel();
        // Send the unconfirmed transaction to the primary.
        self.tx_unconfirmed_transaction.send((transaction_id, transaction, callback_sender)).await?;
        // Await the callback to continue.
        callback_receiver.await?
    }
}

#[derive(Debug)]
pub struct PrimaryReceiver<N: Network> {
    pub rx_unconfirmed_solution: mpsc::Receiver<(SolutionID<N>, Data<Solution<N>>, oneshot::Sender<Result<()>>)>,
    pub rx_unconfirmed_transaction:
        mpsc::Receiver<(N::TransactionID, Data<Transaction<N>>, oneshot::Sender<Result<()>>)>,
}

/// Initializes the primary channels.
pub fn init_primary_channels<N: Network>() -> (PrimarySender<N>, PrimaryReceiver<N>) {
    let (tx_unconfirmed_solution, rx_unconfirmed_solution) = mpsc::channel(MAX_CHANNEL_SIZE);
    let (tx_unconfirmed_transaction, rx_unconfirmed_transaction) = mpsc::channel(MAX_CHANNEL_SIZE);

    let sender = PrimarySender { tx_unconfirmed_solution, tx_unconfirmed_transaction };
    let receiver = PrimaryReceiver { rx_unconfirmed_solution, rx_unconfirmed_transaction };

    (sender, receiver)
}
