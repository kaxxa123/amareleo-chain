// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at:

// http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::path::PathBuf;

use amareleo_chain_tracing::TracingHandler;

/// Amareleo logging selection
#[derive(Clone)]
pub enum AmareleoLog {
    /// Log to file.
    ///     - If Some, the specified path is used.
    ///     - If None, the default log file path is used.
    File(Option<PathBuf>),

    /// Custom log handler.
    ///
    /// Overrides the default log handler by providing a
    /// `TracingHandler` configured with tracing subscribers.
    Custom(TracingHandler),

    /// Disable logging
    None,
}

impl AmareleoLog {
    /// Get configured log file path
    ///
    /// # Returns
    ///
    /// `Option<PathBuf>` - `Some(PathBuf)` when a custom log file path is configured.
    /// `None` when using the default path and when file logging is disabled.
    pub fn get_path(&self) -> Option<PathBuf> {
        match self {
            Self::File(path) => path.clone(),
            _ => None,
        }
    }

    /// Check if file logging is enabled
    ///
    /// # Returns
    ///
    /// Returns `true` when file logging is enabled, `false` otherwise.
    pub fn is_file(&self) -> bool {
        matches!(self, Self::File(_))
    }

    /// Check if custom logging is enabled
    ///
    /// # Returns
    ///
    /// Returns `true` when custom logging is enabled, `false` otherwise.
    pub fn is_custom(&self) -> bool {
        matches!(self, Self::Custom(_))
    }

    /// Check if logging is disabled altogether
    ///
    /// # Returns
    ///
    /// Returns `true` when logging is disabled, `false` otherwise.
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}
