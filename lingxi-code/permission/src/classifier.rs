//! Classifier kinds — full impl in Plan 03 Task 6 (Tools System).
//!
//! For M1, [`ClassifierKind`] lives in `result` to avoid a forward cycle.
//! This module re-exports it for the canonical `classifier::ClassifierKind` path.

pub use crate::result::ClassifierKind;
