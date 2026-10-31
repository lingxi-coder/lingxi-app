//! Product host identity and endpoint eligibility for Anthropic requests.
//!
//! Both the CLI and desktop bridge resolve the same policy before passing the
//! resulting metadata to the runtime's provider, protocol and URL checks.

#![forbid(unsafe_code)]

/// Explicit custom-endpoint opt-in for Native-compatible Anthropic request
/// metadata. The value must match the configured API base; ordinary custom
/// gateways keep LingXi's normal transport policy.
pub const NATIVE_ANTHROPIC_ENDPOINT_ENV: &str = "LINGXI_NATIVE_ANTHROPIC_ENDPOINT";

/// Build the host identity supplied to the Anthropic Native request policy.
///
/// The canonical Anthropic API is enabled by default for first-party provider
/// selection. A custom API base requires an exact endpoint opt-in through
/// [`NATIVE_ANTHROPIC_ENDPOINT_ENV`]. The runtime still checks the resolved
/// provider, protocol, profile base, and request URL on every call, so a
/// different configured provider never receives this policy.
#[must_use]
pub fn native_anthropic_client_metadata_for_host(
    api_base: &str,
    app_id: &str,
    host_version: &str,
) -> Option<lingxi_llm_client::providers::anthropic::request_policy::AnthropicClientMetadata> {
    let first_party_environment = first_party_environment(|name| std::env::var(name).ok());
    let explicit_endpoint = std::env::var(NATIVE_ANTHROPIC_ENDPOINT_ENV).ok();
    native_anthropic_client_metadata_for_host_with_policy(
        api_base,
        app_id,
        host_version,
        first_party_environment,
        explicit_endpoint.as_deref(),
    )
}

// Keep process-environment reads outside the policy calculation so all flags
// can be checked deterministically without changing a test's global environment.
fn first_party_environment(read_env: impl Fn(&str) -> Option<String>) -> bool {
    ![
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "CLAUDE_CODE_USE_ANTHROPIC_AWS",
        "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
        "CLAUDE_CODE_USE_MANTLE",
    ]
    .iter()
    .any(|name| lingxi_core::host::env::is_env_truthy(read_env(name).as_deref()))
}

fn native_anthropic_client_metadata_for_host_with_policy(
    api_base: &str,
    app_id: &str,
    host_version: &str,
    first_party_environment: bool,
    explicit_endpoint: Option<&str>,
) -> Option<lingxi_llm_client::providers::anthropic::request_policy::AnthropicClientMetadata> {
    const CANONICAL_ENDPOINT: &str = "https://api.anthropic.com";
    let configured_base = api_base.trim_end_matches('/');
    let canonical_endpoint = configured_base == CANONICAL_ENDPOINT;
    let explicitly_enabled = explicit_endpoint
        .map(str::trim)
        .filter(|endpoint| !endpoint.is_empty())
        .is_some_and(|endpoint| endpoint.trim_end_matches('/') == configured_base);
    if !first_party_environment || (!canonical_endpoint && !explicitly_enabled) {
        return None;
    }

    let (os, arch) = native_client_platform();
    Some(
        lingxi_llm_client::providers::anthropic::request_policy::AnthropicClientMetadata {
            language: "rust".into(),
            runtime: "lingxi".into(),
            package_version: lingxi_llm_client::PACKAGE_VERSION.into(),
            runtime_version: host_version.into(),
            os,
            arch,
            timeout_seconds: 600,
            session_id: None,
            app_id: app_id.into(),
        },
    )
}

fn native_client_platform() -> (String, String) {
    let os = match std::env::consts::OS {
        "macos" => "MacOS".to_owned(),
        "windows" => "Windows".to_owned(),
        "linux" => "Linux".to_owned(),
        "freebsd" => "FreeBSD".to_owned(),
        "openbsd" => "OpenBSD".to_owned(),
        "android" => "Android".to_owned(),
        "ios" => "iOS".to_owned(),
        other => format!("Other:{other}"),
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" | "arm64" => "arm64".to_owned(),
        "x86_64" | "x64" => "x64".to_owned(),
        "x32" | "arm" => std::env::consts::ARCH.to_owned(),
        other => format!("other:{other}"),
    };
    (os, arch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_and_explicit_endpoints_obey_the_same_host_policy() {
        let canonical = native_anthropic_client_metadata_for_host_with_policy(
            "https://api.anthropic.com/",
            "lingxi-cli",
            "0.12.0",
            true,
            None,
        )
        .expect("the canonical first-party API is enabled");
        assert_eq!(canonical.language, "rust");
        assert_eq!(canonical.runtime, "lingxi");
        assert_eq!(
            canonical.package_version,
            lingxi_llm_client::PACKAGE_VERSION
        );
        assert_eq!(canonical.runtime_version, "0.12.0");
        assert_eq!(canonical.app_id, "lingxi-cli");
        assert_eq!(canonical.timeout_seconds, 600);
        assert_eq!(canonical.session_id, None);

        let custom_base = "http://127.0.0.1:3000/";
        for opt_in in [None, Some(""), Some("  "), Some("http://127.0.0.1:4000")] {
            assert!(native_anthropic_client_metadata_for_host_with_policy(
                custom_base,
                "lingxi-desktop",
                "0.9.0",
                true,
                opt_in,
            )
            .is_none());
        }
        let custom = native_anthropic_client_metadata_for_host_with_policy(
            custom_base,
            "lingxi-desktop",
            "0.9.0",
            true,
            Some(" http://127.0.0.1:3000 "),
        )
        .expect("an exact custom-endpoint opt-in is accepted");
        assert_eq!(custom.app_id, "lingxi-desktop");
        assert_eq!(custom.runtime_version, "0.9.0");

        for (base, opt_in) in [
            ("https://api.anthropic.com", None),
            (custom_base, Some("http://127.0.0.1:3000")),
        ] {
            assert!(
                native_anthropic_client_metadata_for_host_with_policy(
                    base,
                    "lingxi-desktop",
                    "0.9.0",
                    false,
                    opt_in,
                )
                .is_none(),
                "non-first-party environment must override endpoint eligibility"
            );
        }
    }

    #[test]
    fn every_alternate_provider_flag_disables_first_party_metadata_when_truthy() {
        let flags = [
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_USE_FOUNDRY",
            "CLAUDE_CODE_USE_ANTHROPIC_AWS",
            "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
            "CLAUDE_CODE_USE_MANTLE",
        ];
        assert!(first_party_environment(|_| None));
        for flag in flags {
            assert!(
                !first_party_environment(|name| { (name == flag).then(|| "  YeS  ".to_owned()) }),
                "{flag}"
            );
            assert!(
                first_party_environment(|name| { (name == flag).then(|| "false".to_owned()) }),
                "{flag}"
            );
        }
    }
}
