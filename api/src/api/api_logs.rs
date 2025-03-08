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

/// Amareleo logging selection
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AmareleoLog {
    /// Log only to a file
    File(Option<PathBuf>),
    /// Log to both stdout and a file
    All(Option<PathBuf>),
    /// Disable logging
    None,
}

impl AmareleoLog {
    /// Get the path to the log file
    pub fn get_path(&self) -> &Option<PathBuf> {
        match self {
            Self::File(path) | Self::All(path) => path,
            _ => &None,
        }
    }

    /// Check if logging to stdout
    pub fn is_stdout(&self) -> bool {
        matches!(self, Self::All(_))
    }

    /// Check if logging is disabled
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}
