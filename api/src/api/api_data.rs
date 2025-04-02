// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at:

// http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{AmareleoApiState, AmareleoLog, ValidatorApi};

use anyhow::{Result, bail};
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use amareleo_chain_tracing::{TracingHandler, initialize_tracing};
use amareleo_node_bft::helpers::{amareleo_log_file, custom_ledger_dir, default_ledger_dir, endpoint_file_tag};

// Private (only crate-wide visibility) lock-free AmareleoApi data storage
pub(crate) struct AmareleoApiData {
    pub state: AmareleoApiState,
    pub prev_state: AmareleoApiState,
    pub network_id: u16,
    pub rest_ip: SocketAddr,
    pub rest_rps: u32,
    pub keep_state: bool,
    pub ledger_base_path: Option<PathBuf>,
    pub ledger_default_naming: bool,
    pub log_mode: AmareleoLog,
    pub tracing: Option<TracingHandler>,
    pub verbosity: u8,
    pub shutdown: Arc<AtomicBool>,
    pub validator: ValidatorApi,
}

impl AmareleoApiData {
    // Check if state matches the one required
    pub fn is_state(&self, state: AmareleoApiState) -> bool {
        self.state == state
    }

    /// Get log file path, based on current configuration.
    pub fn get_log_file(&self) -> Result<PathBuf> {
        if !self.log_mode.is_file() {
            bail!("Logging to file is disabled!");
        }

        let log_option = self.log_mode.get_path();
        let log_path = match &log_option {
            Some(path) => path.clone(),
            None => {
                let end_point_tag = endpoint_file_tag(false, &self.rest_ip)?;
                amareleo_log_file(self.network_id, self.keep_state, &end_point_tag)
            }
        };

        Ok(log_path)
    }

    // Get ledger folder path, based on current configuration.
    pub fn get_ledger_folder(&self) -> Result<PathBuf> {
        let tag = endpoint_file_tag(self.ledger_default_naming, &self.rest_ip)?;
        let ledger_path = match &self.ledger_base_path {
            Some(base_path) => custom_ledger_dir(self.network_id, self.keep_state, &tag, base_path.clone()),
            None => default_ledger_dir(self.network_id, self.keep_state, &tag),
        };

        Ok(ledger_path.to_path_buf())
    }

    // Initialze logging
    pub fn trace_init(&mut self) -> Result<()> {
        // Determine the effective TraceHandler
        self.tracing = match &self.log_mode {
            AmareleoLog::File(_) => Some(initialize_tracing(self.verbosity, self.get_log_file()?)?),
            AmareleoLog::Custom(tracing) => Some(tracing.clone()),
            AmareleoLog::None => None,
        };

        Ok(())
    }
}
