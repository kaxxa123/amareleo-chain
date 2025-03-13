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
use std::sync::Arc;
use tracing::subscriber::DefaultGuard;
use tracing_subscriber::{Layer, Registry, layer::Layered, prelude::*};

/// Manages tracing subscribers with dynamic layer composition
#[derive(Clone)]
pub struct TracingHandler {
    subscriber: Arc<dyn tracing::Subscriber + Send + Sync + 'static>,
}

impl TracingHandler {
    /// Creates a new TracingHandler with a default Registry subscriber
    pub fn new() -> Self {
        Self { subscriber: Arc::new(Registry::default()) }
    }

    /// Set one layer subscriber
    pub fn set_one_layer<L>(&mut self, layer1: L) -> Result<()>
    where
        L: Layer<Registry> + Send + Sync + 'static,
    {
        // Compose the subscriber with both layers
        let inner_subscriber = tracing_subscriber::registry().with(layer1);
        self.subscriber = Arc::new(inner_subscriber);
        Ok(())
    }

    /// Set two layer subscriber
    pub fn set_two_layers<L, LL>(&mut self, layer1: L, layer2: LL) -> Result<()>
    where
        L: Layer<Registry> + Send + Sync + 'static,
        LL: Layer<Layered<L, Registry>> + Send + Sync + 'static,
    {
        // Compose the subscriber with both layers
        let inner_subscriber = tracing_subscriber::registry().with(layer1).with(layer2);
        self.subscriber = Arc::new(inner_subscriber);
        Ok(())
    }

    /// Set subscriber as the default for the current thread
    pub fn subscribe_thread(&self) -> DefaultGuard {
        tracing::subscriber::set_default(self.subscriber.clone())
    }
}

impl Default for TracingHandler {
    fn default() -> Self {
        Self::new()
    }
}
