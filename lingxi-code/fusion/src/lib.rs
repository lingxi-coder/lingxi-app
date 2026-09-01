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
mod model_resolver;
mod orchestrator;
mod panel;
mod progress;
mod synthesizer;

pub use budget::{FusionPriceBook, FusionQuote, ModelRates};
pub use config::FusionRuntimeConfig;
pub use model_resolver::{CatalogModel, ModelSource, ResolvedPanel, ResolvedSet};
pub use orchestrator::FusionOrchestrator;

#[cfg(test)]
mod orchestrator_test;
