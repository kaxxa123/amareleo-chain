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

use crate::helpers::{DynamicFormatter, LogWriter};

use anyhow::{Result, bail};
use crossterm::tty::IsTty;
use std::{
    fs::File,
    io,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
use tracing_subscriber::{
    EnvFilter,
    layer::{Layer, SubscriberExt},
    util::SubscriberInitExt,
};

/// Initializes a log filter based on the verbosity level.
fn init_env_filter(verbosity: u8) -> Result<EnvFilter> {
    let filter = EnvFilter::from_default_env()
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

fn init_log_file(logfile_path: PathBuf) -> Result<File> {
    // Create the directories tree for a logfile if it doesn't exist.
    if let Some(logfile_dir) = logfile_path.parent() {
        if !logfile_dir.exists() && std::fs::create_dir_all(logfile_dir).is_err() {
            bail!("Failed to create directories, check user has permissions: {}", logfile_dir.to_string_lossy());
        }
    } else {
        bail!("Root directory passed as a logfile");
    }

    // Create a file to write logs to.
    let logfile = File::options().append(true).create(true).open(logfile_path.clone());
    match logfile {
        Ok(file) => Ok(file),
        Err(_) => bail!("Failed to open log file {}", logfile_path.to_string_lossy()),
    }
}

/// Initializes the logger.
///
/// ```ignore
/// 0 => info
/// 1 => info, debug
/// 2 => info, debug, trace, amareleo_node_sync=trace
/// 3 => info, debug, trace, amareleo_node_bft=trace
/// ```
pub fn initialize_logger(
    verbosity: u8,
    logfile_path: PathBuf,
    stdout_dump: bool,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    match verbosity {
        0 => std::env::set_var("RUST_LOG", "info"),
        1 => std::env::set_var("RUST_LOG", "debug"),
        2.. => std::env::set_var("RUST_LOG", "trace"),
    };

    // Initialize tracing to file.
    let trace_to_file = tracing_subscriber::fmt::Layer::default()
        .with_ansi(false)
        .with_writer(init_log_file(logfile_path)?)
        .with_target(verbosity > 2)
        .with_filter(init_env_filter(verbosity)?);

    let subscriber = tracing_subscriber::registry().with(trace_to_file);

    let _ = if stdout_dump {
        // Initialize the log sender.
        // Hardcoding to None as a result of
        // snarkos start --nodisplay being always true.
        let log_sender = None;

        // Add layer using LogWriter for stdout / terminal
        let trace_to_stdout = tracing_subscriber::fmt::Layer::default()
            .with_ansi(io::stdout().is_tty())
            .with_writer(move || LogWriter::new(&log_sender))
            .with_target(verbosity > 2)
            .event_format(DynamicFormatter::new(shutdown))
            .with_filter(init_env_filter(verbosity)?);

        subscriber.with(trace_to_stdout).try_init()
    } else {
        subscriber.try_init()
    };

    Ok(())
}
