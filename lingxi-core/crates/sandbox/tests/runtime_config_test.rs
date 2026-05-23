use lingxi_sandbox::runtime_config::{
    FilesystemRestrictionConfig, NetworkRestrictionConfig, Platform, RipgrepConfig,
    SandboxRuntimeConfig,
};
use std::collections::HashMap;

#[test]
fn default_config_serializes_with_camelcase_keys() {
    let cfg = SandboxRuntimeConfig::default();
    let v = serde_json::to_value(&cfg).expect("serialize default");
    // Every field name from claude-code's zod SandboxSettingsSchema must appear.
    for key in [
        "enabled",
        "failIfUnavailable",
        "enabledPlatforms",
        "autoAllowBashIfSandboxed",
        "allowUnsandboxedCommands",
        "network",
        "filesystem",
        "ignoreViolations",
        "enableWeakerNestedSandbox",
        "enableWeakerNetworkIsolation",
        "excludedCommands",
        "ripgrep",
    ] {
        assert!(
            v.as_object().unwrap().contains_key(key),
            "missing zod key {key} from default config: {v}"
        );
    }
}

#[test]
fn network_subkeys_match_zod_schema() {
    let cfg = SandboxRuntimeConfig::default();
    let v = serde_json::to_value(&cfg).unwrap();
    let net = &v["network"];
    for key in [
        "allowedDomains",
        "allowManagedDomainsOnly",
        "allowUnixSockets",
        "allowAllUnixSockets",
        "allowLocalBinding",
        "httpProxyPort",
        "socksProxyPort",
    ] {
        assert!(
            net.as_object().unwrap().contains_key(key),
            "missing network key {key}: {net}"
        );
    }
}

#[test]
fn filesystem_subkeys_match_zod_schema() {
    let cfg = SandboxRuntimeConfig::default();
    let v = serde_json::to_value(&cfg).unwrap();
    let fs = &v["filesystem"];
    for key in [
        "allowWrite",
        "denyWrite",
        "denyRead",
        "allowRead",
        "allowManagedReadPathsOnly",
    ] {
        assert!(
            fs.as_object().unwrap().contains_key(key),
            "missing filesystem key {key}: {fs}"
        );
    }
}

#[test]
fn deserializes_realistic_claude_code_settings_fragment() {
    let json = serde_json::json!({
        "enabled": true,
        "failIfUnavailable": false,
        "enabledPlatforms": ["mac", "linux"],
        "autoAllowBashIfSandboxed": true,
        "allowUnsandboxedCommands": ["docker"],
        "network": {
            "allowedDomains": ["github.com", "*.anthropic.com"],
            "allowManagedDomainsOnly": false,
            "allowUnixSockets": ["/var/run/docker.sock"],
            "allowAllUnixSockets": false,
            "allowLocalBinding": true,
            "httpProxyPort": 8080,
            "socksProxyPort": 1080
        },
        "filesystem": {
            "allowWrite": ["./build", "./target"],
            "denyWrite": ["/etc"],
            "denyRead": ["/private/etc"],
            "allowRead": ["~/.cargo/registry"],
            "allowManagedReadPathsOnly": false
        },
        "ignoreViolations": { "fs.read": ["~/.cache"] },
        "enableWeakerNestedSandbox": false,
        "enableWeakerNetworkIsolation": false,
        "excludedCommands": ["bazel", "make"],
        "ripgrep": { "command": "/usr/bin/rg", "args": ["--no-config"] }
    });
    let cfg: SandboxRuntimeConfig =
        serde_json::from_value(json).expect("parse SandboxRuntimeConfig");
    assert!(cfg.enabled);
    assert_eq!(
        cfg.enabled_platforms.as_deref(),
        Some(&[Platform::Mac, Platform::Linux][..])
    );
    assert_eq!(cfg.allow_unsandboxed_commands, vec!["docker".to_string()]);
    assert_eq!(
        cfg.network.allowed_domains,
        vec!["github.com".to_string(), "*.anthropic.com".to_string()]
    );
    assert_eq!(cfg.network.http_proxy_port, Some(8080));
    assert_eq!(cfg.filesystem.allow_write, vec!["./build", "./target"]);
    assert_eq!(cfg.excluded_commands, vec!["bazel", "make"]);
    assert_eq!(cfg.ripgrep.command, "/usr/bin/rg");
    assert_eq!(cfg.ripgrep.args, vec!["--no-config"]);
    let _: &HashMap<String, Vec<String>> = &cfg.ignore_violations;
}

#[test]
fn platform_enum_serializes_lowercase() {
    assert_eq!(
        serde_json::to_value(Platform::Mac).unwrap(),
        serde_json::json!("mac")
    );
    assert_eq!(
        serde_json::to_value(Platform::Linux).unwrap(),
        serde_json::json!("linux")
    );
    assert_eq!(
        serde_json::to_value(Platform::Wsl).unwrap(),
        serde_json::json!("wsl")
    );
}

#[test]
fn unused_helpers_are_referenced() {
    // Touch the optional sub-structs to keep them in scope and ensure they're
    // constructible without arguments.
    let _: NetworkRestrictionConfig = NetworkRestrictionConfig::default();
    let _: FilesystemRestrictionConfig = FilesystemRestrictionConfig::default();
    let _: RipgrepConfig = RipgrepConfig::default();
}
