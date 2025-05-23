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

mod clean;
pub use clean::*;

mod start;
pub use start::*;

mod update;
pub use update::*;

use anstyle::{AnsiColor, Color, Style};
use anyhow::Result;
use clap::{Parser, builder::Styles};

const HEADER_COLOR: Option<Color> = Some(Color::Ansi(AnsiColor::Yellow));
const LITERAL_COLOR: Option<Color> = Some(Color::Ansi(AnsiColor::Green));
const STYLES: Styles = Styles::plain()
    .header(Style::new().bold().fg_color(HEADER_COLOR))
    .usage(Style::new().bold().fg_color(HEADER_COLOR))
    .literal(Style::new().bold().fg_color(LITERAL_COLOR));

#[derive(Debug, Parser)]
#[clap(styles = STYLES, version)]
pub struct CLI {
    /// Specify the verbosity [options: 0, 1, 2, 3]
    #[clap(default_value = "2", short, long)]
    pub verbosity: u8,
    /// Specify a subcommand.
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Debug, Parser)]
pub enum Command {
    #[clap(name = "clean")]
    Clean(Clean),
    #[clap(name = "start")]
    Start(Box<Start>),
    #[clap(name = "update")]
    Update(Update),
}

impl Command {
    /// Parses the command.
    pub fn parse(self, repo_name: &str, bin_name: &str) -> Result<String> {
        match self {
            Self::Clean(command) => command.parse(),
            Self::Start(command) => command.parse(),
            Self::Update(command) => command.parse(repo_name, bin_name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // As per the official clap recommendation.
    #[test]
    fn verify_cli() {
        use clap::CommandFactory;
        CLI::command().debug_assert()
    }
}
