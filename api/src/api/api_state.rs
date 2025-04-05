// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at:

// http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

/// AmareleoApi object state enumeration
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmareleoApiState {
    /// Node instance created and is fully configurable.
    /// - State entered on successfully calling `new()` for the first time.
    ///
    /// - Allowed function calls:
    ///     - Config getters: `get_*()` and `is_*()`
    ///     - Config setters: `cfg_*()` and `try_cfg_*()`
    ///     - Node starting: `start()`
    ///
    /// - State transitions from `Init`:
    ///     - Config getters: `Init -> Init`
    ///     - Config setters: `Init -> Init`
    ///     - `start()`: `Init -> StartPending -> Started`
    Init,

    /// Node configuration is completely locked and node is
    /// ready to start.
    ///
    /// This is an intermediate internal state the node goes
    /// through when `start()` is called. However this is not
    /// the final state resulting from calling `start()`. Thus
    /// unless one polls the node state while `start()` is
    /// running, this state may be difficult to detect.
    ///
    /// - State entered on successfully calling `start()`
    ///
    /// - Allowed function calls:
    ///     - Config getters: `get_*()` and `is_*()`
    ///
    /// - State transitions from `StartPending`:
    ///     - Config getters: `StartPending -> StartPending`
    ///     - `start()`: `StartPending -> Started`
    StartPending,

    /// Node configuration is completely locked and node is
    /// running.
    ///
    /// This is the final node state after successfully running
    /// `start()`
    ///
    /// - State entered on successfully calling `start()`
    ///
    /// - Allowed function calls:
    ///     - Config getters: `get_*()` and `is_*()`
    ///     - Node stopping: `end()`
    ///
    /// - State transitions from `Started`:
    ///     - Config getters: `Started -> Started`
    ///     - `end()`: `Started -> StopPending -> Stopped`
    Started,

    /// Node configuration is completely locked and node is
    /// ready to stop.
    ///
    /// This is an intermediate internal state the node goes
    /// through when `end()` is called. However this is not
    /// the final state resulting from calling `end()`. Thus
    /// unless one polls the node state while `end()` is
    /// running, this state may be difficult to detect.
    ///
    /// - State entered on successfully calling `end()`
    ///
    /// - Allowed function calls:
    ///     - Config getters: `get_*()` and `is_*()`
    ///
    /// - State transitions from `StopPending`:
    ///     - Config getters: `StopPending -> StopPending`
    ///     - `end()`: `StopPending -> Stopped`
    StopPending,

    /// Node configuration is __partially__ locked and node is
    /// stopped.
    ///
    /// This is the final node state after successfully running
    /// `end()`
    ///
    /// - State entered on successfully calling `end()`
    ///
    /// - Allowed function calls:
    ///     - Config getters: `get_*()` and `is_*()`
    ///     - Subset of config setters: `cfg_*()` and `try_cfg_*()`
    ///     - Node starting: `start()`
    ///
    /// - State transitions from `Stopped`:
    ///     - Config getters: `Stopped -> Stopped`
    ///     - Config setters: `Stopped -> Stopped`
    ///     - `start()`: `Stopped -> StartPending -> Started`
    Stopped,
}
