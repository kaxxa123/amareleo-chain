// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at:

// http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::{Result, bail};
use std::{fs::File, path::PathBuf};
use tracing_subscriber::{EnvFilter, fmt::Layer as FmtLayer, layer::Layer};

use crate::TracingHandler;

/// Initializes a log filter based on the verbosity level.
/// ```ignore
/// 0 => info
/// 1 => info, debug
/// 2 => info, debug, trace, amareleo_node_sync=trace
/// 3 => info, debug, trace, amareleo_node_bft=trace
/// ```
pub fn init_env_filter(verbosity: u8) -> Result<EnvFilter> {
    let filter = match verbosity {
        0 => EnvFilter::new("info"),
        1 => EnvFilter::new("debug"),
        2.. => EnvFilter::new("trace"),
    };

    let filter = filter
        .add_directive("mio=off".parse()?)
        .add_directive("tokio_util=off".parse()?)
        .add_directive("hyper=off".parse()?)
        .add_directive("reqwest=off".parse()?)
        .add_directive("want=off".parse()?)
        .add_directive("warp=off".parse()?);

    let filter = if verbosity >= 2 {
        filter.add_directive("amareleo_node_sync=trace".parse()?)
    } else {
        filter.add_directive("amareleo_node_sync=debug".parse()?)
    };

    if verbosity >= 3 {
        Ok(filter.add_directive("amareleo_node_bft=trace".parse()?))
    } else {
        Ok(filter.add_directive("amareleo_node_bft=debug".parse()?))
    }
}

/// Open log file creating directories if needed.
pub fn init_file_writer(logfile_path: PathBuf) -> Result<File> {
    // Create the directories tree for a logfile if it doesn't exist.
    match logfile_path.parent() {
        Some(logfile_dir) => {
            if !logfile_dir.exists() && std::fs::create_dir_all(logfile_dir).is_err() {
                bail!("Failed to create directories, check user has permissions: {}", logfile_dir.to_string_lossy());
            }
        }

        None => bail!("Root directory passed as a logfile"),
    }

    // Create a file to write logs to.
    let open_res = File::options().append(true).create(true).open(logfile_path.clone());
    match open_res {
        Ok(file_handle) => Ok(file_handle),
        Err(_) => bail!("Failed to open log file {}", logfile_path.to_string_lossy()),
    }
}

/// Initializes logger.
pub fn initialize_tracing(verbosity: u8, logfile_path: PathBuf) -> Result<TracingHandler> {
    let trace_to_file = FmtLayer::default()
        .with_ansi(false)
        .with_writer(init_file_writer(logfile_path)?)
        .with_target(verbosity > 2)
        .with_filter(init_env_filter(verbosity)?);

    let mut tracing = TracingHandler::new();
    tracing.set_one_layer(trace_to_file)?;
    Ok(tracing)
}
