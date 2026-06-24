//! Minimal, focused port of codex's config-requirements constraint scaffolding,
//! scoped to the **compute-use** requirement.
//!
//! Ported 1:1 (in shape and semantics) from
//! `codex-rs/config/src/config_requirements.rs` and
//! `codex-rs/config/src/constraint.rs`, but deliberately limited to the few
//! generic types that [`ComputerUseRequirementsToml`] actually references:
//!
//! - [`RequirementSource`] — provenance of a managed requirement (used in error
//!   messages).
//! - [`Constrained`] / [`ConstraintError`] / [`ConstraintResult`] — the generic
//!   allow-set constraint helper.
//! - [`Sourced`] — a value paired with the [`RequirementSource`] it came from.
//! - [`ComputerUseRequirementsToml`] — the actual compute-use requirement
//!   (`allow_locked_computer_use`).
//!
//! The full permission / sandbox / hooks / execpolicy / mcp_types stack that
//! codex's `config_requirements.rs` also hosts is intentionally NOT vendored —
//! LingXi has its own permission/sandbox/hooks crates.

#![forbid(unsafe_code)]

mod constraint;

pub use constraint::Constrained;
pub use constraint::ConstraintError;
pub use constraint::ConstraintResult;

use serde::Deserialize;
use std::fmt;
use std::path::PathBuf;

/// Provenance of a managed configuration requirement, surfaced in
/// constraint-violation error messages.
///
/// Ported from codex `config_requirements::RequirementSource`. The file-backed
/// variants use [`PathBuf`] here (codex uses its bespoke `AbsolutePathBuf`),
/// which keeps the shape faithful without pulling codex's absolute-path crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequirementSource {
    Unknown,
    MdmManagedPreferences {
        domain: String,
        key: String,
    },
    /// Multiple requirements layers contributed to the final value. Sources are
    /// stored highest-priority first, matching the order surfaced in errors.
    Composite {
        sources: Vec<RequirementSource>,
    },
    /// A backend-delivered enterprise-managed layer. `id` is the stable backend
    /// identifier; `name` is the admin-facing display name.
    EnterpriseManaged {
        id: String,
        name: String,
    },
    SystemRequirementsToml {
        file: PathBuf,
    },
    LegacyManagedConfigTomlFromFile {
        file: PathBuf,
    },
    LegacyManagedConfigTomlFromMdm,
}

impl RequirementSource {
    pub fn composite(sources: impl IntoIterator<Item = RequirementSource>) -> Self {
        let mut flattened = Vec::new();
        for source in sources {
            source.append_to_composite(&mut flattened);
        }

        match flattened.len() {
            0 => RequirementSource::Unknown,
            1 => flattened.remove(0),
            _ => RequirementSource::Composite { sources: flattened },
        }
    }

    fn append_to_composite(self, flattened: &mut Vec<RequirementSource>) {
        match self {
            RequirementSource::Composite { sources } => {
                for source in sources {
                    source.append_to_composite(flattened);
                }
            }
            source => {
                if !flattened.contains(&source) {
                    flattened.push(source);
                }
            }
        }
    }
}

impl fmt::Display for RequirementSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequirementSource::Unknown => write!(f, "<unspecified>"),
            RequirementSource::MdmManagedPreferences { domain, key } => {
                write!(f, "MDM {domain}:{key}")
            }
            RequirementSource::Composite { sources } => {
                write!(f, "requirements layers: ")?;
                for (index, source) in sources.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{source}")?;
                }
                Ok(())
            }
            RequirementSource::EnterpriseManaged { id, name } => {
                write!(f, "enterprise-managed requirements {name} ({id})")
            }
            RequirementSource::SystemRequirementsToml { file } => {
                write!(f, "{}", file.display())
            }
            RequirementSource::LegacyManagedConfigTomlFromFile { file } => {
                write!(f, "{}", file.display())
            }
            RequirementSource::LegacyManagedConfigTomlFromMdm => {
                write!(f, "MDM managed_config.toml (legacy)")
            }
        }
    }
}

/// Value paired with the requirement source it came from, for better error
/// messages.
///
/// Ported from codex `config_requirements::Sourced`.
#[derive(Debug, Clone, PartialEq)]
pub struct Sourced<T> {
    pub value: T,
    pub source: RequirementSource,
}

impl<T> Sourced<T> {
    pub fn new(value: T, source: RequirementSource) -> Self {
        Self { value, source }
    }
}

impl<T> std::ops::Deref for Sourced<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

/// Compute-use requirement layer.
///
/// Ported from codex `config_requirements::ComputerUseRequirementsToml`.
#[derive(Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct ComputerUseRequirementsToml {
    pub allow_locked_computer_use: Option<bool>,
}

impl ComputerUseRequirementsToml {
    pub fn is_empty(&self) -> bool {
        self.allow_locked_computer_use.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use pretty_assertions::assert_eq;
    use toml::from_str;

    /// Mirrors codex's `deserialize_computer_use_requirements`, narrowed to the
    /// compute-use requirement table since this crate does not vendor the full
    /// `ConfigRequirementsToml`.
    #[test]
    fn deserialize_computer_use_requirements() -> Result<()> {
        let computer_use: ComputerUseRequirementsToml = from_str(
            r#"
                allow_locked_computer_use = false
            "#,
        )?;

        assert_eq!(
            computer_use,
            ComputerUseRequirementsToml {
                allow_locked_computer_use: Some(false),
            }
        );
        assert!(!computer_use.is_empty());
        Ok(())
    }

    #[test]
    fn computer_use_requirements_empty_when_unset() -> Result<()> {
        let computer_use: ComputerUseRequirementsToml = from_str("")?;
        assert_eq!(computer_use, ComputerUseRequirementsToml::default());
        assert!(computer_use.is_empty());
        Ok(())
    }

    #[test]
    fn sourced_wraps_computer_use_requirement() {
        let computer_use = ComputerUseRequirementsToml {
            allow_locked_computer_use: Some(true),
        };
        let sourced = Sourced::new(
            computer_use.clone(),
            RequirementSource::LegacyManagedConfigTomlFromMdm,
        );
        assert_eq!(sourced.value, computer_use);
        // Deref reaches the inner value's fields.
        assert_eq!(sourced.allow_locked_computer_use, Some(true));
    }

    #[test]
    fn composite_requirement_source_flattens_and_deduplicates_sources() {
        let mdm_source = RequirementSource::MdmManagedPreferences {
            domain: "com.openai.codex".to_string(),
            key: "requirements_toml_base64".to_string(),
        };
        let legacy_source = RequirementSource::LegacyManagedConfigTomlFromMdm;

        assert_eq!(
            RequirementSource::composite([
                mdm_source.clone(),
                RequirementSource::composite([legacy_source.clone(), mdm_source.clone()]),
            ]),
            RequirementSource::Composite {
                sources: vec![mdm_source, legacy_source],
            }
        );
    }
}
