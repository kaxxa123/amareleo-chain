// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at:

// http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::Arc;
use tracing::subscriber::DefaultGuard;
use tracing_subscriber::{Layer, Registry, layer::Layered, prelude::*};

pub trait TracingHandlerGuard {
    fn get_tracing_guard(&self) -> Option<DefaultGuard>;
}

/// Manages tracing subscribers with dynamic layer composition
#[derive(Clone)]
pub struct TracingHandler {
    subscriber: Arc<dyn tracing::Subscriber + Send + Sync + 'static>,
    enabled: bool,
}

impl std::fmt::Debug for TracingHandler {
    fn fmt(&self, _f: &mut std::fmt::Formatter) -> std::fmt::Result {
        Ok(())
    }
}

impl TracingHandler {
    /// Creates a new TracingHandler with a default Registry subscriber
    pub fn new() -> Self {
        Self { subscriber: Arc::new(Registry::default()), enabled: false }
    }

    /// Set one layer subscriber
    pub fn set_one_layer<L>(layer1: L) -> Self
    where
        L: Layer<Registry> + Send + Sync + 'static,
    {
        // Compose the subscriber with both layers
        let inner_subscriber = tracing_subscriber::registry().with(layer1);
        Self { subscriber: Arc::new(inner_subscriber), enabled: true }
    }

    /// Set two layer subscriber
    pub fn set_two_layers<L, LL>(layer1: L, layer2: LL) -> Self
    where
        L: Layer<Registry> + Send + Sync + 'static,
        LL: Layer<Layered<L, Registry>> + Send + Sync + 'static,
    {
        // Compose the subscriber with both layers
        let inner_subscriber = tracing_subscriber::registry().with(layer1).with(layer2);
        Self { subscriber: Arc::new(inner_subscriber), enabled: true }
    }
}

impl Default for TracingHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Option<TracingHandler>> for TracingHandler {
    fn from(opt: Option<TracingHandler>) -> Self {
        opt.unwrap_or_default()
    }
}

impl TracingHandlerGuard for TracingHandler {
    /// Set subscriber as the default for the current thread
    fn get_tracing_guard(&self) -> Option<DefaultGuard> {
        if self.enabled { Some(tracing::subscriber::set_default(self.subscriber.clone())) } else { None }
    }
}
