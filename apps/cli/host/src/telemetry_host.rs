//! CLI-owned telemetry identity and the lifetime of one submitted request.

use std::collections::BTreeMap;
use telemetry::otel::{Attributes, ConfigValue, ConfigValueSource, OtelConfig};

pub(crate) fn resolve_config(managed: &BTreeMap<String, String>) -> OtelConfig {
    config_from_lookup(|key| {
        managed
            .get(key)
            .map(|value| ConfigValue {
                value: value.clone(),
                source: ConfigValueSource::Managed,
            })
            .or_else(|| {
                std::env::var(key).ok().map(|value| ConfigValue {
                    value,
                    source: ConfigValueSource::Runtime,
                })
            })
    })
}

fn config_from_lookup(get: impl Fn(&str) -> Option<ConfigValue>) -> OtelConfig {
    OtelConfig::from_lookup_with_source(|key| {
        let value = get(key);
        if key == "OTEL_SERVICE_NAME"
            && value
                .as_ref()
                .is_none_or(|value| value.value.trim().is_empty())
        {
            Some(ConfigValue {
                value: "lingxi-cli".into(),
                source: ConfigValueSource::Runtime,
            })
        } else {
            value
        }
    })
}

/// Poll only this submission under its request span. Creating the span inside
/// the spawned task keeps later queue drains and idle session time outside it.
pub(crate) async fn with_request_span<T>(
    kind: &'static str,
    future: impl std::future::Future<Output = T>,
) -> T {
    telemetry::otel::with_operation_span(
        "lingxi.cli.request",
        Attributes::from([("request.kind".into(), kind.into())]),
        future,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_cli_identity_without_enabling_reporting() {
        let config = config_from_lookup(|_| None);
        assert_eq!(config.service_name, "lingxi-cli");
        assert!(!config.enabled);
    }

    #[test]
    fn preserves_explicit_managed_service_identity() {
        let config = config_from_lookup(|key| {
            (key == "OTEL_SERVICE_NAME").then(|| ConfigValue {
                value: "lingxi-cli-integration".into(),
                source: ConfigValueSource::Managed,
            })
        });
        assert_eq!(config.service_name, "lingxi-cli-integration");
    }

    #[test]
    fn empty_service_name_uses_cli_identity() {
        let config = config_from_lookup(|key| {
            (key == "OTEL_SERVICE_NAME").then(|| ConfigValue {
                value: "  ".into(),
                source: ConfigValueSource::Runtime,
            })
        });
        assert_eq!(config.service_name, "lingxi-cli");
    }
}
