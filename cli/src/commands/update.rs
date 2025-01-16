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

use crate::helpers::Updater;

use anyhow::Result;
use clap::Parser;

/// Update amareleo-chain.
#[derive(Debug, Parser)]
pub struct Update {
    /// Lists all available versions of amareleo-chain
    #[clap(short = 'l', long)]
    list: bool,
    /// Suppress outputs to terminal
    #[clap(short = 'q', long)]
    quiet: bool,
    /// Update to specified version
    #[clap(short = 'v', long)]
    version: Option<String>,
}

impl Update {
    /// Update amareleo-chain.
    pub fn parse(self) -> Result<String> {
        match self.list {
            true => match Updater::show_available_releases() {
                Ok(output) => Ok(output),
                Err(error) => Ok(format!(
                    "Failed to list the available versions of amareleo-chain\n{error}\n"
                )),
            },
            false => {
                let result = Updater::update_to_release(!self.quiet, self.version);
                if !self.quiet {
                    match result {
                        Ok(status) => {
                            if status.uptodate() {
                                Ok("\namareleo-chain is already on the latest version".to_string())
                            } else if status.updated() {
                                Ok(format!(
                                    "\namareleo-chain has updated to version {}",
                                    status.version()
                                ))
                            } else {
                                Ok(String::new())
                            }
                        }
                        Err(e) => Ok(format!(
                            "\nFailed to update amareleo-chain to the latest version\n{e}\n"
                        )),
                    }
                } else {
                    Ok(String::new())
                }
            }
        }
    }
}
