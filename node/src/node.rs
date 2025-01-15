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

use crate::{NodeInterface, Validator};
use snarkos_lite_account::Account;
use snarkos_lite_node_router::messages::NodeType;

use anyhow::Result;
use snarkvm::prelude::{
    block::Block, store::helpers::rocksdb::ConsensusDB, Address, Network, PrivateKey, ViewKey,
};

use aleo_std::StorageMode;
use std::{
    net::SocketAddr,
    sync::{atomic::AtomicBool, Arc},
};

pub struct Node<N: Network> {
    /// The account private key.
    validator: Arc<Validator<N, ConsensusDB<N>>>,
}

impl<N: Network> Node<N> {
    pub async fn new_validator(
        node_ip: SocketAddr,
        rest_ip: Option<SocketAddr>,
        rest_rps: u32,
        account: Account<N>,
        genesis: Block<N>,
        storage_mode: StorageMode,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        Ok(Self {
            validator: Arc::new(
                Validator::new(
                    node_ip,
                    rest_ip,
                    rest_rps,
                    account,
                    genesis,
                    storage_mode,
                    shutdown,
                )
                .await?,
            ),
        })
    }

    /// Returns the node type.
    pub fn node_type(&self) -> NodeType {
        self.validator.node_type()
    }

    /// Returns the account private key of the node.
    pub fn private_key(&self) -> &PrivateKey<N> {
        self.validator.private_key()
    }

    /// Returns the account view key of the node.
    pub fn view_key(&self) -> &ViewKey<N> {
        self.validator.view_key()
    }

    /// Returns the account address of the node.
    pub fn address(&self) -> Address<N> {
        self.validator.address()
    }
}
