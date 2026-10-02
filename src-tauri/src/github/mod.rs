//! GitHub data layer: the PR model, the GraphQL query documents, and the
//! mapping from raw GraphQL JSON to typed Rust.

mod advisory;
pub mod client;
pub mod gates;
pub mod map;
pub mod model;
pub mod mutate;
pub mod query;
pub mod ready_stacks;
pub mod stack;
pub mod stack_merge;
pub mod stats;

pub mod admission;
mod read_transport;

pub(crate) mod scan;
