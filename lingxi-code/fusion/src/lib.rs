//! Fusion multi-model deliberation orchestrator.
//!
//! This crate implements the Fusion state machine. Callers depend on
//! [`platform_api::FusionExecutor`]; composition roots inject a
//! [`FusionOrchestrator`]. There is no public Agent / slash / workflow entry
//! here — those land in later PRs.

#![forbid(unsafe_code)]

mod analyst;
mod budget;
mod config;
mod decision;
pub mod evidence;
mod model_resolver;
mod orchestrator;
mod packing;
mod panel;
mod progress;
mod snapshot;
mod synthesizer;

pub use budget::{CapturedPriceBook, FusionPriceBook, FusionQuote, ModelRates};
pub use config::{FusionConfigSource, FusionRuntimeConfig};
pub use model_resolver::{CatalogModel, ModelLimits, ModelSource, ResolvedPanel, ResolvedSet};
pub use orchestrator::FusionOrchestrator;
pub use snapshot::{CatalogRevision, CatalogSnapshot, FusionRuntimeSnapshot};

#[cfg(test)]
mod orchestrator_test;
