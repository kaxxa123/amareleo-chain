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
#![allow(clippy::blocks_in_conditions)]
#![allow(clippy::type_complexity)]

#[macro_use]
extern crate tracing;

pub use amareleo_node_bft_events as events;
pub use amareleo_node_bft_ledger_service as ledger_service;
pub use amareleo_node_bft_storage_service as storage_service;

pub mod helpers;

mod bft;
pub use bft::*;

mod primary;
pub use primary::*;

mod sync;
pub use sync::*;

mod worker;
pub use worker::*;

/// The maximum number of milliseconds to wait before proposing a batch.
pub const MAX_BATCH_DELAY_IN_MS: u64 = 2000; // ms
/// The minimum number of seconds to wait before proposing a batch.
pub const MIN_BATCH_DELAY_IN_SECS: u64 = 1; // seconds
/// The maximum number of seconds allowed for the leader to send their certificate.
pub const MAX_LEADER_CERTIFICATE_DELAY_IN_SECS: i64 = 2 * MAX_BATCH_DELAY_IN_MS as i64 / 1000; // seconds
/// The maximum number of seconds before the timestamp is considered expired.
pub const MAX_TIMESTAMP_DELTA_IN_SECS: i64 = 10; // seconds
/// The maximum number of workers that can be spawned.
pub const MAX_WORKERS: u8 = 1; // worker(s)
/// The development mode RNG seed.
pub const DEVELOPMENT_MODE_RNG_SEED: u64 = 1234567890u64;
/// The development mode number of genesis committee members.
pub const DEVELOPMENT_MODE_NUM_GENESIS_COMMITTEE_MEMBERS: u16 = 4;

/// A helper macro to spawn a blocking task.
#[macro_export]
macro_rules! spawn_blocking {
    ($expr:expr) => {
        match tokio::task::spawn_blocking(move || $expr).await {
            Ok(value) => value,
            Err(error) => Err(anyhow::anyhow!("[tokio::spawn_blocking] {error}")),
        }
    };
}
