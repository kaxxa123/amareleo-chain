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

use colored::Colorize;
use self_update::{Status, backends::github, version::bump_is_greater};
use std::fmt::Write;

pub struct Updater;

impl Updater {
    const AMARELEOCHAIN_REPO_OWNER: &'static str = "kaxxa123";

    /// Show all available releases.
    pub fn show_available_releases(repo_name: &str) -> Result<String, UpdaterError> {
        let releases = github::ReleaseList::configure()
            .repo_owner(Self::AMARELEOCHAIN_REPO_OWNER)
            .repo_name(repo_name)
            .build()?
            .fetch()?;

        let mut output = "List of available versions\n".to_string();
        for release in releases {
            let _ = writeln!(output, "  * {}", release.version);
        }
        Ok(output)
    }

    /// Update to the specified release.
    pub fn update_to_release(
        show_output: bool,
        repo_name: &str,
        bin_name: &str,
        version: Option<String>,
    ) -> Result<Status, UpdaterError> {
        let mut update_builder = github::Update::configure();

        update_builder
            .repo_owner(Self::AMARELEOCHAIN_REPO_OWNER)
            .repo_name(repo_name)
            .bin_name(bin_name)
            .current_version(env!("CARGO_PKG_VERSION"))
            .show_download_progress(show_output)
            .no_confirm(true)
            .show_output(show_output);

        let status = match version {
            None => update_builder.build()?.update()?,
            Some(new_version) => {
                // Normalize version to have a leading 'v'
                let new_version = if new_version.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                    format!("v{new_version}")
                } else {
                    new_version
                };

                update_builder.target_version_tag(&new_version).build()?.update()?
            }
        };

        Ok(status)
    }

    /// Check if there is an available update and return the newest release.
    pub fn update_available(repo_name: &str, bin_name: &str) -> Result<String, UpdaterError> {
        let updater = github::Update::configure()
            .repo_owner(Self::AMARELEOCHAIN_REPO_OWNER)
            .repo_name(repo_name)
            .bin_name(bin_name)
            .current_version(env!("CARGO_PKG_VERSION"))
            .build()?;

        let current_version = updater.current_version();
        let latest_release = updater.get_latest_release()?;

        if bump_is_greater(&current_version, &latest_release.version)? {
            Ok(latest_release.version)
        } else {
            Err(UpdaterError::OldReleaseVersion(current_version, latest_release.version))
        }
    }

    /// Display the CLI message.
    pub fn print_cli(repo_name: &str, bin_name: &str) -> String {
        if let Ok(latest_version) = Self::update_available(repo_name, bin_name) {
            let mut output = "🟢 A new version is available! Run".bold().green().to_string();
            output += &format!(" `{bin_name} update` ").bold().white();
            output += &format!("to update to v{latest_version}.").bold().green();
            output
        } else {
            String::new()
        }
    }
}

#[derive(Debug, Error)]
pub enum UpdaterError {
    #[error("{}: {}", _0, _1)]
    Crate(&'static str, String),

    #[error("The current version {} is more recent than the release version {}", _0, _1)]
    OldReleaseVersion(String, String),
}

impl From<self_update::errors::Error> for UpdaterError {
    fn from(error: self_update::errors::Error) -> Self {
        UpdaterError::Crate("self_update", error.to_string())
    }
}
