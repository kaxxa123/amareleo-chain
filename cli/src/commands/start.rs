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

use anyhow::Result;
use clap::Parser;
use colored::Colorize;

use amareleo_api::api::{AmareleoApi, AmareleoLog};

use snarkvm::{
    console::network::{CanaryV0, MainnetV0, Network, TestnetV0},
    prelude::store::helpers::rocksdb::ConsensusDB,
};
use std::result::Result::Ok;

use std::{net::SocketAddr, path::PathBuf};
use tokio::runtime::{self, Runtime};

/// Starts the node.
#[derive(Clone, Debug, Parser)]
pub struct Start {
    /// Specify the network ID of this node
    #[clap(default_value = "1", long = "network")]
    pub network: u16,

    /// Specify the IP address and port for the REST server [default: 127.0.0.1:3030]
    #[clap(long = "rest")]
    pub rest: Option<SocketAddr>,
    /// Specify the REST server requests per second (RPS) rate limit per IP
    #[clap(default_value = "10", long = "rest-rps")]
    pub rest_rps: u32,

    /// Specify the verbosity level [options: 0, 1, 2, 3, 4]
    #[clap(default_value = "1", long = "verbosity")]
    pub verbosity: u8,
    /// Specify the path to the log file
    #[clap(long = "logfile")]
    pub logfile: Option<PathBuf>,

    /// Enables the metrics exporter
    #[clap(default_value = "false", long = "metrics")]
    pub metrics: bool,
    /// Specify the IP address and port for the metrics exporter [default: 127.0.0.1:9000]
    #[clap(long = "metrics-ip")]
    pub metrics_ip: Option<SocketAddr>,

    /// Specify the path to the ledger storage directory [default: current directory]
    #[clap(long = "storage")]
    pub storage: Option<PathBuf>,

    /// Enables preserving the chain state across runs
    #[clap(default_value = "false", long = "keep-state")]
    pub keep_state: bool,
}

impl Start {
    /// Starts the node.
    pub fn parse(self) -> Result<String> {
        // Initialize the runtime.
        Self::runtime().block_on(async move {
            // Clone the configurations.
            let mut cli = self.clone();
            // Parse the network.
            match cli.network {
                MainnetV0::ID => {
                    // Parse the node from the configurations.
                    cli.parse_node::<MainnetV0>().await.expect("Failed to parse the node");
                }
                TestnetV0::ID => {
                    // Parse the node from the configurations.
                    cli.parse_node::<TestnetV0>().await.expect("Failed to parse the node");
                }
                CanaryV0::ID => {
                    // Parse the node from the configurations.
                    cli.parse_node::<CanaryV0>().await.expect("Failed to parse the node");
                }
                _ => panic!("Invalid network ID specified"),
            };
            // Note: Do not move this. The pending await must be here otherwise
            // other snarkOS commands will not exit.
            std::future::pending::<()>().await;
        });

        Ok(String::new())
    }

    /// Returns the node type corresponding to the given configurations.
    #[rustfmt::skip]
    async fn parse_node<N: Network>(&mut self) -> Result<()> {

        // Print the welcome.
        println!("{}", Self::welcome_message());

        // Parse the REST IP.
        let rest_ip_port = self.rest.as_ref().copied().unwrap_or_else(|| "0.0.0.0:3030".parse().unwrap());

        // Get the node account.
        let account = AmareleoApi::<N>::get_node_account()?;
        println!("🔑 Your development private key for node 0 is {}.\n", account.private_key().to_string().bold());
        println!("👛 Your Aleo address is {}.\n", account.address().to_string().bold());
        println!("🧭 Starting node on {}.\n",N::NAME.bold());
        println!("🌐 Starting the REST server at {}.\n", rest_ip_port.to_string().bold());

        if let Ok(jwt_token) = amareleo_node_rest::Claims::new(account.address()).to_jwt_string() {
            println!("🔑 Your one-time JWT token is {}\n", jwt_token.dimmed());
        }

        // Initialize the metrics.
        if self.metrics {
            metrics::initialize_metrics(self.metrics_ip);
        }

        let mut node_api: AmareleoApi<N> = AmareleoApi::default();

        node_api
        .cfg_ledger(self.keep_state, self.storage.clone(), self.keep_state)
        .cfg_rest(rest_ip_port, self.rest_rps)
        .cfg_log(AmareleoLog::All(self.logfile.clone()), self.verbosity);

        println!("📝 Log file path: {}\n", node_api.get_log_file()?.to_string_lossy());
        println!("📁 Ledger folder path: {}\n", node_api.get_ledger_folder()?.to_string_lossy());

        // Initialize the node.
        node_api.start::<ConsensusDB<N>>().await?;
        Ok(())
    }

    /// Returns a runtime for the node.
    fn runtime() -> Runtime {
        // Retrieve the number of cores.
        let num_cores = num_cpus::get();

        // Initialize the number of tokio worker threads, max tokio blocking threads, and rayon cores.
        // Note: We intentionally set the number of tokio worker threads and number of rayon cores to be
        // more than the number of physical cores, because the node is expected to be I/O-bound.
        let (num_tokio_worker_threads, max_tokio_blocking_threads, num_rayon_cores_global) =
            (2 * num_cores, 512, num_cores);

        // Initialize the parallelization parameters.
        rayon::ThreadPoolBuilder::new()
            .stack_size(8 * 1024 * 1024)
            .num_threads(num_rayon_cores_global)
            .build_global()
            .unwrap();

        // Initialize the runtime configuration.
        runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(8 * 1024 * 1024)
            .worker_threads(num_tokio_worker_threads)
            .max_blocking_threads(max_tokio_blocking_threads)
            .build()
            .expect("Failed to initialize a runtime for the router")
    }

    /// Returns the welcome message as a string.
    fn welcome_message() -> String {
        use colored::Colorize;

        let mut output = String::new();
        output += &r#"

         ╦╬╬╬╬╬╦
        ╬╬╬╬╬╬╬╬╬                    ▄▄▄▄        ▄▄▄
       ╬╬╬╬╬╬╬╬╬╬╬                  ▐▓▓▓▓▌       ▓▓▓
      ╬╬╬╬╬╬╬╬╬╬╬╬╬                ▐▓▓▓▓▓▓▌      ▓▓▓     ▄▄▄▄▄▄       ▄▄▄▄▄▄
     ╬╬╬╬╬╬╬╬╬╬╬╬╬╬╬              ▐▓▓▓  ▓▓▓▌     ▓▓▓   ▄▓▓▀▀▀▀▓▓▄   ▐▓▓▓▓▓▓▓▓▌
    ╬╬╬╬╬╬╬╜ ╙╬╬╬╬╬╬╬            ▐▓▓▓▌  ▐▓▓▓▌    ▓▓▓  ▐▓▓▓▄▄▄▄▓▓▓▌ ▐▓▓▓    ▓▓▓▌
   ╬╬╬╬╬╬╣     ╠╬╬╬╬╬╬           ▓▓▓▓▓▓▓▓▓▓▓▓    ▓▓▓  ▐▓▓▀▀▀▀▀▀▀▀▘ ▐▓▓▓    ▓▓▓▌
  ╬╬╬╬╬╬╣       ╠╬╬╬╬╬╬         ▓▓▓▓▌    ▐▓▓▓▓   ▓▓▓   ▀▓▓▄▄▄▄▓▓▀   ▐▓▓▓▓▓▓▓▓▌
 ╬╬╬╬╬╬╣         ╠╬╬╬╬╬╬       ▝▀▀▀▀      ▀▀▀▀▘  ▀▀▀     ▀▀▀▀▀▀       ▀▀▀▀▀▀
╚╬╬╬╬╬╩           ╩╬╬╬╬╩


"#
        .white()
        .bold();
        output += &"👋 Welcome to Aleo! We thank you for running a node and supporting privacy.\n".bold();
        output
    }
}
