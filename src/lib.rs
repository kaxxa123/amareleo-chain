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

use amareleo_chain_cli::{commands::CLI, helpers::Updater};

use clap::Parser;
use std::{env, process::exit};

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
use tikv_jemallocator::Jemalloc;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

pub struct BuildInfo<'a> {
    pub bin: &'a str,
    pub version: &'a str,
    pub repo: &'a str,
    pub branch: &'a str,
    pub commit: &'a str,
    pub features: &'a str,
}

pub fn main_core(build: &BuildInfo) -> anyhow::Result<()> {
    // A hack to avoid having to go through clap to display advanced version information.
    check_for_version(build);

    // Parse the given arguments.
    let cli = CLI::parse();
    // Run the updater.
    println!("{}", Updater::print_cli(build.repo, build.bin));
    // Run the CLI.
    match cli.command.parse(build.repo, build.bin) {
        Ok(output) => println!("{output}\n"),
        Err(error) => {
            // Print the top level error and then any additional context.
            println!("⚠️  {error}");
            for entry in error.chain().skip(1) {
                println!("     ↳ {entry}");
            }
            println!();
            exit(1);
        }
    }
    Ok(())
}

/// Checks whether the version information was requested and - if so - display it and exit.
fn check_for_version(build: &BuildInfo) {
    if let Some(first_arg) = env::args().nth(1) {
        if ["--version", "-V"].contains(&&*first_arg) {
            println!("{} {} {} {} features=[{}]", build.bin, build.version, build.branch, build.commit, build.features);
            exit(0);
        }
    }
}
