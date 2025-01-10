use crate::helpers::Proposal;
use parking_lot::RwLock;

/// A helper type for an optional proposed batch.
pub type ProposedBatch<N> = RwLock<Option<Proposal<N>>>;
