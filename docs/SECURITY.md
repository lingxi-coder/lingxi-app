# Security model

## Secrets
- `Secret<T>` (wrapper around `secrecy::SecretBox<T>`) zeroizes on drop.
- `Debug`/`Display` of `Secret<T>` and `SecureStorageData` always emit `<redacted>`.
- The only access path is `expose_secret()`, which is grep-able for audit.
- `SecureStorage` trait abstracts over Keychain/libsecret/Cred Vault/Keystore/PlainText.

## Sandbox
- `ProcessRunner::run` accepts only `SandboxedCommand`.
- `SandboxedCommand` is only constructible via `Sandbox::prepare` (policy applied)
  or `Sandbox::bypass_with_audit` (reason recorded).
- Sandbox canonicalizes paths and rejects symlink escape (A2).
- `should_use_sandbox` decision logic refuses dangerous bash commands when
  sandbox is unavailable.

## Permission
- 8 rule sources with explicit priority.
- `DenialTrackingState` falls back to prompt after threshold per tool.
- `PermissionResult::Ask` carries an optional `pending_classifier_check` that
  can race the user prompt.
- `bypass_killswitch_active` overrides `BypassPermissions` mode.

## OAuth
- Loopback HTTP listener bound only to 127.0.0.1.
- PKCE S256 code verifier/challenge.
- State token validated on callback (CSRF defense).
- Token storage via `SecureStorage`.

## Plugins
- Default trust for git/local plugins is `Untrusted` (A7).
- Plugin agent frontmatter cannot set `permission_mode`/`hooks`/`mcpServers`.
- Sensitive `user_config` fields resolved through `CredentialManager`, never
  stored in plugin manifest plain text.

## Hooks
- SSRF guard blocks RFC1918/loopback IPs by default.
- Hook `Command` executor on mobile is refused at registration time (M3 gap).

## IDE bridge
- 8-char alphanumeric pairing codes excluding visually confusable characters.
- Pairing rate-limited per project (token-bucket).
- JWT tokens are project-scoped: a token issued for project A cannot
  authenticate against project B.

## Telemetry
- PII markers (`Verified`, `PiiTagged`) are real newtypes; the type system
  forces explicit assertion at call sites.
- `_PROTO_*` keys are stripped before any general-access sink.
