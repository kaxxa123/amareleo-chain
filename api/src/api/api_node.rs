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
    AmareleoApiData,
    AmareleoApiState,
    AmareleoLog,
    ValidatorApi,
    ValidatorNewData,
    clean_tmp_ledger,
    is_valid_network_id,
};

use std::{net::SocketAddr, path::PathBuf, sync::OnceLock};

use anyhow::{Result, bail};
use parking_lot::Mutex;
use snarkvm::console::network::{Network, TestnetV0};

use amareleo_chain_tracing::TracingHandler;
use amareleo_node_bft::helpers::amareleo_storage_mode;

// ===================================================================
// AmareleoApi is a simple thread-safe singleton.
// There should be little scope for accessing this object
// from concurrent threads. Thus we adopt a simple non-rentrant
// Mutex that locks all data both on reading and writing.
//
// The non-rentrant Mutex, requires that functions acquiring
// the Mutex lock do not call other functions that would also lock
// the same Mutex. That would lead to a deadlock.
//
// The general rule is to only let public functions lock the Mutex,
// such that private functions can be safely shared. However this
// rule is not strictly followed. See the start(), end() and try_*()
// functions for example.
//
// The AmareleoApiData struct is a lock-free private space most
// appropriate for reusable code. This struct is private and has no
// access to the Mutex.
//
// ===================================================================

/// Amareleo node object, for creating and managing a node instance
pub struct AmareleoApi {
    data: Mutex<AmareleoApiData>,
}

// Global storage for the singleton instance
static INSTANCE: OnceLock<AmareleoApi> = OnceLock::new();

impl AmareleoApi {
    /// Create or return the singleton instance of AmareleoApi.
    ///
    /// When called for the first time the node is initialized with:
    /// - Testnet network
    /// - Bound to localhost port 3030
    /// - REST limited to 10 requests per second
    /// - No ledger state retenttion across runs
    /// - Unique ledger folder naming
    /// - Logging disabled
    pub fn new() -> &'static Self {
        INSTANCE.get_or_init(|| Self {
            data: Mutex::new(AmareleoApiData {
                state: AmareleoApiState::Init,
                prev_state: AmareleoApiState::Init,
                network_id: TestnetV0::ID,
                rest_ip: "0.0.0.0:3030".parse().unwrap(),
                rest_rps: 10u32,
                keep_state: false,
                ledger_base_path: None,
                ledger_default_naming: false,
                log_mode: AmareleoLog::None,
                tracing: None,
                verbosity: 1u8,
                shutdown: Default::default(),
                validator: ValidatorApi::None,
            }),
        })
    }
}

// AmareleoApi configuration setters
impl AmareleoApi {
    /// Configure network ID. Valid network values include:
    /// - CanaryV0::ID
    /// - TestnetV0::ID
    /// - MainnetV0::ID
    ///
    /// # Parameters
    ///
    /// * network_id: `u16` - network ID to use.
    ///
    /// # Returns
    ///
    /// * `Err` if node is NOT in the `Init` state.
    /// * `Err` if network_id is invalid.
    pub fn cfg_network(&self, network_id: u16) -> Result<()> {
        let mut data = self.data.lock();

        if !data.is_state(AmareleoApiState::Init) {
            bail!("Invalid node state {:?}", data.state);
        }

        if !is_valid_network_id(network_id) {
            bail!("Invalid network id {network_id}");
        }
        data.network_id = network_id;
        Ok(())
    }

    /// Same as `cfg_network` but fails silently.
    ///
    /// # Returns
    ///
    /// `&Self` allowing for chaining `try_cfg_*()` calls
    pub fn try_cfg_network(&self, network_id: u16) -> &Self {
        let _ = self.cfg_network(network_id);
        self
    }

    /// Configure REST server IP, port, and requests per second limit.
    ///
    /// # Parameters
    ///
    /// * ip_port: `SocketAddr` - IP and port to listen to
    /// * rps: `u32` - requests per second limit
    ///
    /// # Returns
    ///
    /// * `Err` if node is NOT in the `Init` state.
    pub fn cfg_rest(&self, ip_port: SocketAddr, rps: u32) -> Result<()> {
        let mut data = self.data.lock();

        if !data.is_state(AmareleoApiState::Init) {
            bail!("Invalid node state {:?}", data.state);
        }
        data.rest_ip = ip_port;
        data.rest_rps = rps;
        Ok(())
    }

    /// Same as `cfg_rest` but fails silently.
    ///
    /// # Returns
    ///
    /// `&Self` allowing for chaining `try_cfg_*()` calls
    pub fn try_cfg_rest(&self, ip_port: SocketAddr, rps: u32) -> &Self {
        let _ = self.cfg_rest(ip_port, rps);
        self
    }

    /// Configure ledger storage properties.
    ///
    /// # Parameters
    ///
    /// * keep_state: `bool` - keep ledger state between restarts
    /// * base_path: `Option<PathBuf>` - custom ledger folder path
    /// * default_naming: `bool` - if `true` use fixed ledger folder naming, instead of deriving a unique name from the rest IP:port configuration.
    ///
    /// # Returns
    ///
    /// * `Err` if node is NOT in the `Init` state.
    pub fn cfg_ledger(&self, keep_state: bool, base_path: Option<PathBuf>, default_naming: bool) -> Result<()> {
        let mut data = self.data.lock();

        if !data.is_state(AmareleoApiState::Init) {
            bail!("Invalid node state {:?}", data.state);
        }
        data.keep_state = keep_state;
        data.ledger_base_path = base_path;
        data.ledger_default_naming = default_naming;
        Ok(())
    }

    /// Same as `cfg_ledger` but fails silently.
    ///
    /// # Returns
    ///
    /// `&Self` allowing for chaining `try_cfg_*()` calls
    pub fn try_cfg_ledger(&self, keep_state: bool, base_path: Option<PathBuf>, default_naming: bool) -> &Self {
        let _ = self.cfg_ledger(keep_state, base_path, default_naming);
        self
    }

    /// Configure file logging path and verbosity level.
    ///
    /// # Parameters
    ///
    /// * log_file: `Option<PathBuf>` - custom log file path, or None for default path
    /// * verbosity: `u8` - log verbosity level between 0 and 4, where 0 is the least verbose
    ///
    /// # Returns
    ///
    /// * `Err` if node is NOT in the `Init` or `Stopped` state.
    pub fn cfg_file_log(&self, log_file: Option<PathBuf>, verbosity: u8) -> Result<()> {
        let mut data = self.data.lock();

        if !data.is_state(AmareleoApiState::Init) && !data.is_state(AmareleoApiState::Stopped) {
            bail!("Invalid node state {:?}", data.state);
        }
        data.log_mode = AmareleoLog::File(log_file);
        data.verbosity = verbosity;
        Ok(())
    }

    /// Same as `cfg_file_log` but fails silently.
    ///
    /// # Returns
    ///
    /// `&Self` allowing for chaining `try_cfg_*()` calls
    pub fn try_cfg_file_log(&self, log_file: Option<PathBuf>, verbosity: u8) -> &Self {
        let _ = self.cfg_file_log(log_file, verbosity);
        self
    }

    /// Configure custom tracing subscribers.
    ///
    /// # Parameters
    ///
    /// * tracing: `TracingHandler` - custom tracing subscriber
    ///
    /// # Returns
    ///
    /// * `Err` if node is NOT in the `Init` or `Stopped` state.
    pub fn cfg_custom_log(&self, tracing: TracingHandler) -> Result<()> {
        let mut data = self.data.lock();

        if !data.is_state(AmareleoApiState::Init) && !data.is_state(AmareleoApiState::Stopped) {
            bail!("Invalid node state {:?}", data.state);
        }
        data.log_mode = AmareleoLog::Custom(tracing);
        Ok(())
    }

    /// Same as `cfg_custom_log` but fails silently.
    ///
    /// # Returns
    ///
    /// `&Self` allowing for chaining `try_cfg_*()` calls
    pub fn try_cfg_custom_log(&self, tracing: TracingHandler) -> &Self {
        let _ = self.cfg_custom_log(tracing);
        self
    }

    /// Disable logging.
    ///
    /// # Returns
    ///
    /// * `Err` if node is NOT in the `Init` or `Stopped` state.
    pub fn cfg_no_log(&self) -> Result<()> {
        let mut data = self.data.lock();

        if !data.is_state(AmareleoApiState::Init) && !data.is_state(AmareleoApiState::Stopped) {
            bail!("Invalid node state {:?}", data.state);
        }
        data.log_mode = AmareleoLog::None;
        Ok(())
    }

    /// Same as `cfg_no_log` but fails silently.
    ///
    /// # Returns
    ///
    /// `&Self` allowing for chaining `try_cfg_*()` calls
    pub fn try_cfg_no_log(&self) -> &Self {
        let _ = self.cfg_no_log();
        self
    }
}

// AmareleoApi public getters
impl AmareleoApi {
    /// Get the node object state
    ///
    /// # Returns
    ///
    /// * `AmareleoApiState` - one of the enum state values.
    pub fn get_state(&self) -> AmareleoApiState {
        self.data.lock().state
    }

    /// Check if the node is started
    ///
    /// # Returns
    ///
    /// * `bool` - `true` if the node is running, `false` otherwise
    pub fn is_started(&self) -> bool {
        self.data.lock().is_state(AmareleoApiState::Started)
    }

    /// Get log file path, based on current configuration.
    /// Fails if file logging is not configured.
    ///
    /// Note: Changes in configuration may cause the log file path to change.<br>
    /// Properties that effect the log file path include:<br>
    /// - REST endpont
    /// - network ID
    /// - ledger state retention flag
    ///
    /// # Returns
    ///
    /// * `Result<PathBuf>` - log file path
    pub fn get_log_file(&self) -> Result<PathBuf> {
        self.data.lock().get_log_file()
    }

    /// Get ledger folder path, based on the current configuration.
    ///
    /// Note: Changes in configuration may cause the ledger path to change.<br>
    /// Properties that effect the ledger path include:<br>
    /// - REST endpont (if default naming is disabled)
    /// - network ID
    /// - ledger state retention flag
    ///
    /// # Returns
    ///
    /// * `Result<PathBuf>` - ledger file path
    pub fn get_ledger_folder(&self) -> Result<PathBuf> {
        self.data.lock().get_ledger_folder()
    }
}

// AmareleoApi node start operation
impl AmareleoApi {
    /// Start node instance
    ///
    /// Method should be called when node is in the `Init` or `Stopped` state.<br>
    /// If the node is in any other state, the method fails.
    pub async fn start(&self) -> Result<()> {
        // The state rules are enforced within start_prep/start_complete
        // Sandwiching validator::new between them, secures against
        // concurrency.
        let validator_data = self.start_prep()?;
        let res = ValidatorApi::new(validator_data).await;

        match res {
            Ok(vapi) => {
                self.start_complete(vapi);
            }
            Err(error) => {
                self.start_revert();
                return Err(error);
            }
        };

        Ok(())
    }

    fn start_prep(&self) -> Result<ValidatorNewData> {
        let mut data = self.data.lock();

        if !data.is_state(AmareleoApiState::Init) && !data.is_state(AmareleoApiState::Stopped) {
            bail!("Invalid node state {:?}", data.state);
        }

        let ledger_path = data.get_ledger_folder()?;
        let storage_mode = amareleo_storage_mode(ledger_path.clone());

        data.trace_init()?;

        if !data.keep_state {
            // Clean the temporary ledger.
            clean_tmp_ledger(ledger_path)?;
        }
        data.prev_state = data.state;
        data.state = AmareleoApiState::StartPending;

        Ok(ValidatorNewData {
            network_id: data.network_id,
            rest_ip: data.rest_ip,
            rest_rps: data.rest_rps,
            keep_state: data.keep_state,
            storage_mode: storage_mode.clone(),
            tracing: data.tracing.clone(),
            shutdown: data.shutdown.clone(),
        })
    }

    fn start_complete(&self, validator: ValidatorApi) {
        let mut data = self.data.lock();
        data.validator = validator;
        data.prev_state = data.state;
        data.state = AmareleoApiState::Started;
    }

    fn start_revert(&self) {
        let mut data = self.data.lock();
        data.state = data.prev_state;
    }
}

// AmareleoApi node stop operation
impl AmareleoApi {
    /// Stop node instance
    ///
    /// Method should be called when node is in the `Started` state.<br>
    /// If the node is in any other state, the method fails.
    pub async fn end(&self) -> Result<()> {
        // The state rules are enforced within end_prep/end_complete
        // Sandwiching validator::shut_down between them, secures
        // against concurrency.
        let validator_ = self.end_prep()?;
        validator_.shut_down().await;
        self.end_complete();
        Ok(())
    }

    fn end_prep(&self) -> Result<ValidatorApi> {
        let mut data = self.data.lock();

        if !data.is_state(AmareleoApiState::Started) {
            bail!("Invalid node state {:?}", data.state);
        }
        data.prev_state = data.state;
        data.state = AmareleoApiState::StopPending;

        Ok(data.validator.clone())
    }

    fn end_complete(&self) {
        let mut data = self.data.lock();

        data.validator = ValidatorApi::None;
        data.prev_state = data.state;
        data.state = AmareleoApiState::Stopped;
    }
}
