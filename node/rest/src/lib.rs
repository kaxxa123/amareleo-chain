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

#![forbid(unsafe_code)]

#[macro_use]
extern crate tracing;

mod helpers;
pub use helpers::*;

mod routes;

use amareleo_chain_tracing::TracingHandler;
use amareleo_node_consensus::Consensus;
use snarkvm::{
    console::{program::ProgramID, types::Field},
    prelude::{Ledger, Network, cfg_into_iter, store::ConsensusStorage},
};

use anyhow::{Result, anyhow, bail};
use axum::{
    Json,
    body::Body,
    extract::{ConnectInfo, DefaultBodyLimit, Path, Query, State},
    http::{Method, Request, StatusCode, header::CONTENT_TYPE},
    middleware,
    middleware::Next,
    response::Response,
    routing::{get, post},
};
use axum_extra::response::ErasedJson;
use parking_lot::Mutex;
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    net::TcpListener,
    sync::{
        oneshot,
        oneshot::{Receiver, Sender},
    },
    task::JoinHandle,
};
use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder};
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing::subscriber::DefaultGuard;

/// A REST API server for the ledger.
#[derive(Clone)]
pub struct Rest<N: Network, C: ConsensusStorage<N>> {
    /// The consensus module.
    consensus: Option<Consensus<N>>,
    /// The ledger.
    ledger: Ledger<N, C>,
    /// Tracing handle
    tracing: Option<TracingHandler>,
    /// shutdown signal
    shutdown_tx: Arc<Mutex<Option<Sender<()>>>>,
    /// The server handles.
    rest_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl<N: Network, C: 'static + ConsensusStorage<N>> Rest<N, C> {
    /// Initializes a new instance of the server.
    pub async fn start(
        rest_ip: SocketAddr,
        rest_rps: u32,
        consensus: Option<Consensus<N>>,
        ledger: Ledger<N, C>,
        tracing: Option<TracingHandler>,
    ) -> Result<Self> {
        // Initialize the server.
        let mut server = Self {
            consensus,
            ledger,
            tracing,
            shutdown_tx: Arc::new(Mutex::new(None)),
            rest_handle: Arc::new(Mutex::new(None)),
        };
        // Spawn the server.
        server.spawn_server(rest_ip, rest_rps).await?;
        // Return the server.
        Ok(server)
    }

    pub async fn shut_down(&self) {
        // Extract and replace with None
        let shutdown_option = self.shutdown_tx.lock().take();
        if let Some(tx) = shutdown_option {
            let _ = tx.send(()); // Send shutdown signal
        }

        // Extract and replace with None
        let handle_option = self.rest_handle.lock().take();
        if let Some(handle) = handle_option {
            let _ = handle.await;
        }

        info!("REST server shutdown completed.");
    }
}

impl<N: Network, C: ConsensusStorage<N>> Rest<N, C> {
    /// Returns the ledger.
    pub const fn ledger(&self) -> &Ledger<N, C> {
        &self.ledger
    }

    /// Retruns tracing guard
    pub fn get_tracing_guard(&self) -> Option<DefaultGuard> {
        self.tracing.clone().map(|trace_handle| trace_handle.subscribe_thread())
    }
}

impl<N: Network, C: ConsensusStorage<N>> Rest<N, C> {
    async fn spawn_server(&mut self, rest_ip: SocketAddr, rest_rps: u32) -> Result<()> {
        let _guard = self.get_tracing_guard();

        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers([CONTENT_TYPE]);

        // Log the REST rate limit per IP.
        debug!("REST rate limit per IP - {rest_rps} RPS");

        // Prepare the rate limiting setup.
        let governor_config = Box::new(
            GovernorConfigBuilder::default()
                .per_nanosecond((1_000_000_000 / rest_rps) as u64)
                .burst_size(rest_rps)
                .error_handler(|error| {
                    // Properly return a 429 Too Many Requests error
                    let error_message = error.to_string();
                    let mut response = Response::new(error_message.clone().into());
                    *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                    if error_message.contains("Too Many Requests") {
                        *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
                    }
                    response
                })
                .finish()
                .expect("Couldn't set up rate limiting for the REST server!"),
        );

        // Get the network being used.
        let network = match N::ID {
            snarkvm::console::network::MainnetV0::ID => "mainnet",
            snarkvm::console::network::TestnetV0::ID => "testnet",
            snarkvm::console::network::CanaryV0::ID => "canary",
            unknown_id => bail!("Unknown network ID ({unknown_id})"),
        };

        let router = {
            let routes = axum::Router::new()
                // All the endpoints before the call to `route_layer` are protected with JWT auth.
                .route(
                    &format!("/{network}/node/address"),
                    get(Self::get_node_address),
                )
                .route(
                    &format!("/{network}/program/:id/mapping/:name"),
                    get(Self::get_mapping_values),
                )
                .route_layer(middleware::from_fn(auth_middleware))
                // GET ../block/..
                .route(
                    &format!("/{network}/block/height/latest"),
                    get(Self::get_block_height_latest),
                )
                .route(
                    &format!("/{network}/block/hash/latest"),
                    get(Self::get_block_hash_latest),
                )
                .route(
                    &format!("/{network}/block/latest"),
                    get(Self::get_block_latest),
                )
                .route(
                    &format!("/{network}/block/:height_or_hash"),
                    get(Self::get_block),
                )
                // The path param here is actually only the height, but the name must match the route
                // above, otherwise there'll be a conflict at runtime.
                .route(&format!("/{network}/block/:height_or_hash/header"), get(Self::get_block_header))
                .route(&format!("/{network}/block/:height_or_hash/transactions"), get(Self::get_block_transactions))
                // GET and POST ../transaction/..
                .route(
                    &format!("/{network}/transaction/:id"),
                    get(Self::get_transaction),
                )
                .route(
                    &format!("/{network}/transaction/confirmed/:id"),
                    get(Self::get_confirmed_transaction),
                )
                .route(
                    &format!("/{network}/transaction/broadcast"),
                    post(Self::transaction_broadcast),
                )
                // POST ../solution/broadcast
                .route(
                    &format!("/{network}/solution/broadcast"),
                    post(Self::solution_broadcast),
                )
                // GET ../find/..
                .route(
                    &format!("/{network}/find/blockHash/:tx_id"),
                    get(Self::find_block_hash),
                )
                .route(
                    &format!("/{network}/find/blockHeight/:state_root"),
                    get(Self::find_block_height_from_state_root),
                )
                .route(
                    &format!("/{network}/find/transactionID/deployment/:program_id"),
                    get(Self::find_transaction_id_from_program_id),
                )
                .route(
                    &format!("/{network}/find/transactionID/:transition_id"),
                    get(Self::find_transaction_id_from_transition_id),
                )
                .route(
                    &format!("/{network}/find/transitionID/:input_or_output_id"),
                    get(Self::find_transition_id),
                )
                // GET ../peers/..
                .route(
                    &format!("/{network}/peers/count"),
                    get(Self::get_peers_count),
                )
                .route(&format!("/{network}/peers/all"), get(Self::get_peers_all))
                .route(
                    &format!("/{network}/peers/all/metrics"),
                    get(Self::get_peers_all_metrics),
                )
                // GET ../program/..
                .route(&format!("/{network}/program/:id"), get(Self::get_program))
                .route(
                    &format!("/{network}/program/:id/mappings"),
                    get(Self::get_mapping_names),
                )
                .route(
                    &format!("/{network}/program/:id/mapping/:name/:key"),
                    get(Self::get_mapping_value),
                )
                // GET misc endpoints.
                .route(&format!("/{network}/blocks"), get(Self::get_blocks))
                .route(&format!("/{network}/height/:hash"), get(Self::get_height))
                .route(
                    &format!("/{network}/memoryPool/transmissions"),
                    get(Self::get_memory_pool_transmissions),
                )
                .route(
                    &format!("/{network}/memoryPool/solutions"),
                    get(Self::get_memory_pool_solutions),
                )
                .route(
                    &format!("/{network}/memoryPool/transactions"),
                    get(Self::get_memory_pool_transactions),
                )
                .route(
                    &format!("/{network}/statePath/:commitment"),
                    get(Self::get_state_path_for_commitment),
                )
                .route(
                    &format!("/{network}/stateRoot/latest"),
                    get(Self::get_state_root_latest),
                )
                .route(
                    &format!("/{network}/stateRoot/:height"),
                    get(Self::get_state_root),
                )
                .route(
                    &format!("/{network}/committee/latest"),
                    get(Self::get_committee_latest),
                )
                .route(
                    &format!("/{network}/committee/:height"),
                    get(Self::get_committee),
                )
                .route(
                    &format!("/{network}/delegators/:validator"),
                    get(Self::get_delegators_for_validator),
                );

            // If the `history` feature is enabled, enable the additional endpoint.
            #[cfg(feature = "history")]
            let routes =
                routes.route(&format!("/{network}/block/:blockHeight/history/:mapping"), get(Self::get_history));

            routes
                // Pass in `Rest` to make things convenient.
                .with_state(self.clone())
                // Enable tower-http tracing.
                .layer(TraceLayer::new_for_http())
                // Custom logging.
                .layer(middleware::from_fn_with_state(self.clone(), Self::log_middleware))
                // Enable CORS.
                .layer(cors)
                // Cap body size at 512KiB.
                .layer(DefaultBodyLimit::max(512 * 1024))
                .layer(GovernorLayer {
                    // We can leak this because it is created only once and it persists.
                    config: Box::leak(governor_config),
                })
        };

        // Create a oneshot channel to signal when the server is done
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let rest_listener =
            TcpListener::bind(rest_ip).await.map_err(|err| anyhow!("Failed to bind to {}: {}", rest_ip, err))?;

        let serve_handle = tokio::spawn(async move {
            axum::serve(rest_listener, router.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(Self::shutdown_wait(shutdown_rx))
                .await
                .expect("couldn't start rest server");
        });

        *self.rest_handle.lock() = Some(serve_handle);
        *self.shutdown_tx.lock() = Some(shutdown_tx);

        Ok(())
    }

    async fn log_middleware(
        State(rest): State<Self>,
        ConnectInfo(addr): ConnectInfo<SocketAddr>,
        request: Request<Body>,
        next: Next,
    ) -> Result<Response, StatusCode> {
        let _guard = rest.get_tracing_guard();
        info!("Received '{} {}' from '{addr}'", request.method(), request.uri());

        Ok(next.run(request).await)
    }

    async fn shutdown_wait(shutdown_rx: Receiver<()>) {
        if let Err(error) = shutdown_rx.await {
            error!("REST server shutdown signaling error: {}", error);
        }

        info!("REST server shutdown signal recieved...");
    }
}

/// Formats an ID into a truncated identifier (for logging purposes).
pub fn fmt_id(id: impl ToString) -> String {
    let id = id.to_string();
    let mut formatted_id = id.chars().take(16).collect::<String>();
    if id.chars().count() > 16 {
        formatted_id.push_str("..");
    }
    formatted_id
}
