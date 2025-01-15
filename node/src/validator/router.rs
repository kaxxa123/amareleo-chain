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

use super::*;
use snarkos_lite_node_router::messages::{
    BlockRequest, BlockResponse, DataBlocks, DisconnectReason, Message, MessageCodec, Ping, Pong,
    UnconfirmedTransaction,
};
use snarkos_lite_node_tcp::{Connection, ConnectionSide, Tcp};
use snarkvm::{
    ledger::{narwhal::Data, puzzle::Solution},
    prelude::{block::Header, block::Transaction, error, Network},
};

use std::{io, net::SocketAddr, time::Duration};

impl<N: Network, C: ConsensusStorage<N>> P2P for Validator<N, C> {
    /// Returns a reference to the TCP instance.
    fn tcp(&self) -> &Tcp {
        &self.tcp
    }
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> Handshake for Validator<N, C> {
    /// Performs the handshake protocol.
    async fn perform_handshake(&self, mut connection: Connection) -> io::Result<Connection> {
        Ok(connection)
    }
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> OnConnect for Validator<N, C>
where
    Self: Outbound<N>,
{
    async fn on_connect(&self, peer_addr: SocketAddr) {
        return;
    }
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> Disconnect for Validator<N, C> {
    /// Any extra operations to be performed during a disconnect.
    async fn handle_disconnect(&self, peer_addr: SocketAddr) {}
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> Writing for Validator<N, C> {
    type Codec = MessageCodec<N>;
    type Message = Message<N>;

    /// Creates an [`Encoder`] used to write the outbound messages to the target stream.
    /// The `side` parameter indicates the connection side **from the node's perspective**.
    fn codec(&self, _addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> Reading for Validator<N, C> {
    type Codec = MessageCodec<N>;
    type Message = Message<N>;

    /// Creates a [`Decoder`] used to interpret messages from the network.
    /// The `side` param indicates the connection side **from the node's perspective**.
    fn codec(&self, _peer_addr: SocketAddr, _side: ConnectionSide) -> Self::Codec {
        Default::default()
    }

    /// Processes a message received from the network.
    async fn process_message(
        &self,
        peer_addr: SocketAddr,
        message: Self::Message,
    ) -> io::Result<()> {
        Ok(())
    }
}

impl<N: Network, C: ConsensusStorage<N>> Validator<N, C> {
    async fn process_message_inner(
        &self,
        peer_addr: SocketAddr,
        message: <Validator<N, C> as snarkos_lite_node_tcp::protocols::Reading>::Message,
    ) {
    }
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> Routing<N> for Validator<N, C> {}

impl<N: Network, C: ConsensusStorage<N>> Heartbeat<N> for Validator<N, C> {
    /// The maximum number of peers permitted to maintain connections with.
    const MAXIMUM_NUMBER_OF_PEERS: usize = 200;
}

impl<N: Network, C: ConsensusStorage<N>> Outbound<N> for Validator<N, C> {
    /// Returns a reference to the router.
    fn router(&self) -> &Router<N> {
        &self.router
    }

    /// Returns `true` if the node is synced up to the latest block (within the given tolerance).
    fn is_block_synced(&self) -> bool {
        true
    }

    /// Returns the number of blocks this node is behind the greatest peer height.
    fn num_blocks_behind(&self) -> u32 {
        0u32
    }
}

#[async_trait]
impl<N: Network, C: ConsensusStorage<N>> Inbound<N> for Validator<N, C> {
    /// Retrieves the blocks within the block request range, and returns the block response to the peer.
    fn block_request(&self, peer_ip: SocketAddr, message: BlockRequest) -> bool {
        true
    }

    /// Handles a `BlockResponse` message.
    fn block_response(&self, _peer_ip: SocketAddr, blocks: Vec<Block<N>>) -> bool {
        if !blocks.is_empty() {
            warn!("The sync pool did not request any blocks");
        }
        blocks.is_empty()
    }

    /// Processes the block locators and sends back a `Pong` message.
    fn ping(&self, peer_ip: SocketAddr, _message: Ping<N>) -> bool {
        true
    }

    /// Sleeps for a period and then sends a `Ping` message to the peer.
    fn pong(&self, peer_ip: SocketAddr, _message: Pong) -> bool {
        true
    }

    /// Retrieves the latest epoch hash and latest block header, and returns the puzzle response to the peer.
    fn puzzle_request(&self, peer_ip: SocketAddr) -> bool {
        true
    }

    /// Disconnects on receipt of a `PuzzleResponse` message.
    fn puzzle_response(
        &self,
        peer_ip: SocketAddr,
        _epoch_hash: N::BlockHash,
        _header: Header<N>,
    ) -> bool {
        false
    }

    /// Propagates the unconfirmed solution to all connected validators.
    async fn unconfirmed_solution(
        &self,
        peer_ip: SocketAddr,
        serialized: UnconfirmedSolution<N>,
        solution: Solution<N>,
    ) -> bool {
        true
    }

    /// Handles an `UnconfirmedTransaction` message.
    async fn unconfirmed_transaction(
        &self,
        peer_ip: SocketAddr,
        serialized: UnconfirmedTransaction<N>,
        transaction: Transaction<N>,
    ) -> bool {
        true
    }
}
