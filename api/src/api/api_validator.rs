// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at:

// http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{get_genesis, get_node_account};

use std::{
    net::SocketAddr,
    sync::{Arc, atomic::AtomicBool},
};

use aleo_std::StorageMode;

use snarkvm::{
    console::network::{CanaryV0, MainnetV0, Network, TestnetV0},
    ledger::store::helpers::rocksdb::ConsensusDB,
};

use anyhow::{Ok, Result, bail};

use amareleo_chain_tracing::TracingHandler;
use amareleo_node::Validator;

// Enumeration helping AmareleoApi to handle a Validator
// instances without the need for generic type parameters.
#[derive(Clone)]
pub(crate) enum ValidatorApi {
    CanaryV0(Validator<CanaryV0, ConsensusDB<CanaryV0>>),
    MainnetV0(Validator<MainnetV0, ConsensusDB<MainnetV0>>),
    TestnetV0(Validator<TestnetV0, ConsensusDB<TestnetV0>>),
    None,
}

#[derive(Clone)]
pub(crate) struct ValidatorNewData {
    pub network_id: u16,
    pub rest_ip: SocketAddr,
    pub rest_rps: u32,
    pub keep_state: bool,
    pub storage_mode: StorageMode,
    pub tracing: Option<TracingHandler>,
    pub shutdown: Arc<AtomicBool>,
}

impl ValidatorApi {
    pub async fn new(data: ValidatorNewData) -> Result<Self> {
        match data.network_id {
            CanaryV0::ID => {
                let validator = Validator::<CanaryV0, ConsensusDB<CanaryV0>>::new(
                    data.rest_ip,
                    data.rest_rps,
                    get_node_account::<CanaryV0>()?,
                    get_genesis::<CanaryV0>()?,
                    data.keep_state,
                    data.storage_mode,
                    data.tracing,
                    data.shutdown,
                )
                .await?;
                Ok(ValidatorApi::CanaryV0(validator))
            }
            TestnetV0::ID => {
                let validator = Validator::<TestnetV0, ConsensusDB<TestnetV0>>::new(
                    data.rest_ip,
                    data.rest_rps,
                    get_node_account::<TestnetV0>()?,
                    get_genesis::<TestnetV0>()?,
                    data.keep_state,
                    data.storage_mode,
                    data.tracing,
                    data.shutdown,
                )
                .await?;
                Ok(ValidatorApi::TestnetV0(validator))
            }
            MainnetV0::ID => {
                let validator = Validator::<MainnetV0, ConsensusDB<MainnetV0>>::new(
                    data.rest_ip,
                    data.rest_rps,
                    get_node_account::<MainnetV0>()?,
                    get_genesis::<MainnetV0>()?,
                    data.keep_state,
                    data.storage_mode,
                    data.tracing,
                    data.shutdown,
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
}
