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
#![recursion_limit = "256"]

mod trace_layers;
pub use trace_layers::*;

mod logger;
pub use logger::*;

/// macro to setup info tracing subscribers for structs implementing TracingHandlerGuard
#[macro_export]
macro_rules! guard_info {
    ($guard_expr:expr, $($args:tt)*) => {
        {
            let _guard = (&$guard_expr).get_tracing_guard();
            info!($($args)*);
        }
    };
}

/// macro to setup warn tracing subscribers for structs implementing TracingHandlerGuard
#[macro_export]
macro_rules! guard_warn {
    ($guard_expr:expr, $($args:tt)*) => {
        {
            let _guard = (&$guard_expr).get_tracing_guard();
            warn!($($args)*);
        }
    };
}

/// macro to setup error tracing subscribers for structs implementing TracingHandlerGuard
#[macro_export]
macro_rules! guard_error {
    ($guard_expr:expr, $($args:tt)*) => {
        {
            let _guard = (&$guard_expr).get_tracing_guard();
            error!($($args)*);
        }
    };
}

/// macro to setup debug tracing subscribers for structs implementing TracingHandlerGuard
#[macro_export]
macro_rules! guard_debug {
    ($guard_expr:expr, $($args:tt)*) => {
        {
            let _guard = (&$guard_expr).get_tracing_guard();
            debug!($($args)*);
        }
    };
}

/// macro to setup trace tracing subscribers for structs implementing TracingHandlerGuard
#[macro_export]
macro_rules! guard_trace {
    ($guard_expr:expr, $($args:tt)*) => {
        {
            let _guard = (&$guard_expr).get_tracing_guard();
            trace!($($args)*);
        }
    };
}
