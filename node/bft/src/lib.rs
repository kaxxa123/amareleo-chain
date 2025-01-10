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
extern crate async_trait;
#[macro_use]
extern crate tracing;

pub use snarkos_lite_node_bft_events as events;
pub use snarkos_lite_node_bft_ledger_service as ledger_service;
pub use snarkos_lite_node_bft_storage_service as storage_service;

pub mod helpers;

mod primary;
pub use primary::*;

mod worker;
pub use worker::*;

/// The maximum number of seconds before the timestamp is considered expired.
pub const MAX_TIMESTAMP_DELTA_IN_SECS: i64 = 10; // seconds
/// The maximum number of workers that can be spawned.
pub const MAX_WORKERS: u8 = 1; // worker(s)
