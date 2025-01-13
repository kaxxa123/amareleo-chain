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

use snarkos_lite_node::bft::helpers::{amareleo_ledger_dir, proposal_cache_path};

use anyhow::{bail, Result};
use clap::Parser;
use colored::Colorize;
use std::path::PathBuf;

/// Cleans the snarkOS node storage.
#[derive(Debug, Parser)]
pub struct Clean {
    /// Specify the network to remove from storage.
    #[clap(default_value = "1", long = "network")]
    pub network: u16,
    /// Specify the path to a directory containing the ledger
    #[clap(long = "path")]
    pub path: Option<PathBuf>,
}

impl Clean {
    /// Cleans the snarkOS node storage.
    pub fn parse(self) -> Result<String> {
        // Remove the current proposal cache file, if it exists.
        let proposal_cache_path = proposal_cache_path(self.network, Some(0u16));
        if proposal_cache_path.exists() {
            if let Err(err) = std::fs::remove_file(&proposal_cache_path) {
                bail!(
                    "Failed to remove the current proposal cache file at {}: {err}",
                    proposal_cache_path.display()
                );
            }
        }

        // Remove the specified ledger from storage.
        let ledger_path = match self.path {
            Some(path) => path,
            None => amareleo_ledger_dir(self.network),
        };

        Self::remove_ledger(ledger_path)
    }

    /// Removes the specified ledger from storage.
    pub(crate) fn remove_ledger(path: PathBuf) -> Result<String> {
        // Prepare the path string.
        let path_string = format!("(in \"{}\")", path.display()).dimmed();

        // Check if the path to the ledger exists in storage.
        if path.exists() {
            // Remove the ledger files from storage.
            match std::fs::remove_dir_all(&path) {
                Ok(_) => Ok(format!(
                    "✅ Cleaned the amareleo node storage {path_string}"
                )),
                Err(error) => {
                    bail!(
                        "Failed to remove the amareleo node storage {path_string}\n{}",
                        error.to_string().dimmed()
                    )
                }
            }
        } else {
            Ok(format!(
                "✅ No amareleo node storage was found {path_string}"
            ))
        }
    }
}
