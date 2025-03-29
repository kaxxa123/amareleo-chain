// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at:

// http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
use crate::AmareleoApi;

use std::{
    net::SocketAddr,
    sync::{Arc, atomic::AtomicBool},
};

use aleo_std::StorageMode;

use snarkvm::{
    console::network::{CanaryV0, MainnetV0, Network, TestnetV0},
    ledger::store::helpers::rocksdb::ConsensusDB,
};

use anyhow::{Result, bail};

use amareleo_chain_tracing::TracingHandler;
use amareleo_node::Validator;

// Enumeration helping handling of Validator instances without
// the need for geeneric type parameters.
pub(crate) enum ValidatorApi {
    CanaryV0(Validator<CanaryV0, ConsensusDB<CanaryV0>>),
    MainnetV0(Validator<MainnetV0, ConsensusDB<MainnetV0>>),
    TestnetV0(Validator<TestnetV0, ConsensusDB<TestnetV0>>),
    None,
}

impl ValidatorApi {
    pub async fn new(
        network_id: u16,
        rest_ip: SocketAddr,
        rest_rps: u32,
        keep_state: bool,
        storage_mode: StorageMode,
        log_trace: Option<TracingHandler>,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        match network_id {
            CanaryV0::ID => {
                let validator = Validator::<CanaryV0, ConsensusDB<CanaryV0>>::new(
                    rest_ip,
                    rest_rps,
                    AmareleoApi::get_node_account::<CanaryV0>()?,
                    AmareleoApi::get_genesis::<CanaryV0>()?,
                    keep_state,
                    storage_mode,
                    log_trace,
                    shutdown,
                )
                .await?;
                Ok(ValidatorApi::CanaryV0(validator))
            }
            TestnetV0::ID => {
                let validator = Validator::<TestnetV0, ConsensusDB<TestnetV0>>::new(
                    rest_ip,
                    rest_rps,
                    AmareleoApi::get_node_account::<TestnetV0>()?,
                    AmareleoApi::get_genesis::<TestnetV0>()?,
                    keep_state,
                    storage_mode,
                    log_trace,
                    shutdown,
                )
                .await?;
                Ok(ValidatorApi::TestnetV0(validator))
            }
            MainnetV0::ID => {
                let validator = Validator::<MainnetV0, ConsensusDB<MainnetV0>>::new(
                    rest_ip,
                    rest_rps,
                    AmareleoApi::get_node_account::<MainnetV0>()?,
                    AmareleoApi::get_genesis::<MainnetV0>()?,
                    keep_state,
                    storage_mode,
                    log_trace,
                    shutdown,
                )
                .await?;
                Ok(ValidatorApi::MainnetV0(validator))
            }
            _ => bail!("Unsupported network ID"),
        }
    }

    pub async fn shut_down(&self) {
        match self {
            ValidatorApi::CanaryV0(validator) => validator.shut_down().await,
            ValidatorApi::TestnetV0(validator) => validator.shut_down().await,
            ValidatorApi::MainnetV0(validator) => validator.shut_down().await,
            ValidatorApi::None => (),
        }
    }

    pub fn is_started(&self) -> bool {
        !matches!(self, ValidatorApi::None)
    }
}
