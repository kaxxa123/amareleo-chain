// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at:

// http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::Result;
use crossterm::tty::IsTty;
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
use tracing_subscriber::{fmt::Layer as FmtLayer, layer::Layer};

use crate::helpers::DynamicFormatter;
use amareleo_chain_api::helpers::{TracingHandler, init_env_filter, init_file_writer};

/// Initializes logger.
pub fn initialize_custom_tracing(
    verbosity: u8,
    logfile_path: PathBuf,
    shutdown: Arc<AtomicBool>,
) -> Result<TracingHandler> {
    let trace_to_file = FmtLayer::default()
        .with_ansi(false)
        .with_writer(init_file_writer(logfile_path)?)
        .with_target(verbosity > 2)
        .with_filter(init_env_filter(verbosity)?);

    // Add layer using LogWriter for stdout / terminal
    let stdout_layer = FmtLayer::default()
        .with_ansi(std::io::stdout().is_tty())
        .with_writer(std::io::stdout)
        .with_target(verbosity > 2)
        .event_format(DynamicFormatter::new(shutdown))
        .with_filter(init_env_filter(verbosity)?);

    TracingHandler::new().set_two_layers(trace_to_file, stdout_layer)
}
