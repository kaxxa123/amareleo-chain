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

use amareleo_node::bft::helpers::{
    DEFAULT_FILE_TAG,
    amareleo_storage_mode,
    custom_ledger_dir,
    default_ledger_dir,
    proposal_cache_path,
};
use snarkvm::console::network::{CanaryV0, MainnetV0, Network, TestnetV0};

use anyhow::{Result, bail};
use clap::Parser;
use colored::Colorize;
use std::path::PathBuf;

/// Cleans the node storage.
#[derive(Debug, Parser)]
pub struct Clean {
    /// Specify the network to remove from storage (0 = mainnet, 1 = testnet, 2 = canary)
    #[clap(default_value_t=TestnetV0::ID, long = "network", value_parser = clap::value_parser!(u16).range((MainnetV0::ID as i64)..=(CanaryV0::ID as i64)))]
    pub network: u16,
    /// Specify the path to the ledger storage directory [default: current directory]
    #[clap(long = "path")]
    pub path: Option<PathBuf>,
}

impl Clean {
    /// Cleans the node storage.
    pub fn parse(&self) -> Result<String> {
        self.remove_all(false)?;
        self.remove_all(true)?;

        Ok(String::from("✅ Clean Completed"))
    }

    fn get_file_list(pattern: &str) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = Vec::new();

        match glob::glob(pattern) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(path) => paths.push(path),
                        Err(e) => println!("⚠️  Glob entry error: {}", e),
                    }
                }
            }
            Err(e) => println!("⚠️  Glob pattern error: {}", e),
        }
        paths
    }

    pub fn remove_all(&self, keep_state: bool) -> Result<()> {
        let file_tag = if keep_state { DEFAULT_FILE_TAG } else { "*" };

        // Determine the ledger path
        let pattern = match &self.path {
            Some(path) => custom_ledger_dir(self.network, keep_state, file_tag, path.clone()),
            None => default_ledger_dir(self.network, keep_state, file_tag),
        };

        if let Some(pattern_str) = pattern.to_str() {
            let paths = Self::get_file_list(pattern_str);
            for path in paths {
                // Remove proposal cache file, and ledger
                Self::remove_proposal_cache(path.clone())?;
                let msg = Self::remove_ledger(path)?;
                println!("{msg}");
            }
        } else {
            bail!("Cannot read file pattern!");
        }

        Ok(())
    }

    /// Removes the specified ledger from storage.
    pub(crate) fn remove_proposal_cache(path: PathBuf) -> Result<()> {
        let storage_mode = amareleo_storage_mode(path);

        // Remove the current proposal cache file, if it exists.
        let cache_path = proposal_cache_path(&storage_mode)?;
        if cache_path.exists() {
            if let Err(err) = std::fs::remove_file(&cache_path) {
                bail!("Failed to remove the current proposal cache file at {}: {err}", cache_path.display());
            }
        }

        Ok(())
    }

    /// Removes the specified ledger from storage.
    pub(crate) fn remove_ledger(path: PathBuf) -> Result<String> {
        // Prepare the path string.
        let path_string = format!("(in \"{}\")", path.display()).dimmed();

        // Check if the path to the ledger exists in storage.
        if path.exists() {
            // Remove the ledger files from storage.
            match std::fs::remove_dir_all(&path) {
                Ok(_) => Ok(format!("✅ Cleaned the amareleo node storage {path_string}")),
                Err(error) => {
                    bail!("Failed to remove the amareleo node storage {path_string}\n{}", error.to_string().dimmed())
                }
            }
        } else {
            Ok(format!("✅ No amareleo node storage was found {path_string}"))
        }
    }
}
