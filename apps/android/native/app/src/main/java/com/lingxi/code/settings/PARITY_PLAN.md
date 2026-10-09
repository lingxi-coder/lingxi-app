# Android settings parity (Desktop e0d900b8f)

Preserve existing routes and repository data. Add searchable four-group navigation and a 840dp sidebar. Wire engine settings snapshots and administration through the existing ConversationSource event/command stream. Layered editors must use the selected layer's raw values, show effective/provenance/locks, prohibit managed edits, and await engine acknowledgement. MCP uses independent user/project/local scopes. Keep mobile runtime and device controls reachable.

Baseline: inspect existing settings contracts and run SettingsStoreLocalizationTest, ProviderSettingsSemanticsTest, VoiceSettingsContractTest and LocalAppPluginSettingsTest before changes. Add pure navigation and layer/validation regression tests. Validate compilation and the settings JVM suite; root integration owns full build and visual verification.

## Implemented contracts

- Four desktop groups, localized search, native semantic icons; a single stable NavHost with a 280dp sidebar at 840dp and above.
- Engine source generations isolate commands, replies and drafts. Same-source refresh preserves dirty drafts. Managed locks and broken files reject writes.
- Permissions use dedicated rule/mode/directory commands. Generic settings writes refuse permissions and are confirmed from the selected layer's subsequent snapshot.
- MCP has independent storage scopes and captured revisions. Skills and hooks use revisioned administration. Plugin lifecycle requires an engine preview before confirmation.
- Plugin secrets use the existing Android Keystore adapter and exact Rust SecureStorageData envelope; sensitive values never enter settings patches. Applying/clearing the engine cache requires explicit reconnect.
- Common provider fields use native controls; optional provider metadata/pricing/capabilities use a labeled advanced editor. Full per-profile configuration is editable; bulk LingXi/OpenCode paste and file imports include validation, sanitized previews and conflict selection.
- Existing provider/OAuth connection testing, appearance/language/voice stores, Linux runtime, scheduling and skill surfaces remain available. Old MCP URLs resolve to live administration. Notification/input/privacy settings use actual Android system surfaces. Unsupported mock Dream controls are no longer advertised.

## Verification handoff

Baseline settings JVM suite passed. The expanded settings JVM suite and Compose instrumentation compilation passed during development before the final permission/credential/secret additions. Root coordinates final Direct/Play JVM tests, lint and instrumentation to avoid concurrent Gradle task snapshots. New tests: DesktopSettingsContractTest, PluginSecretRepositoryTest, DesktopSettingsUiTest and PluginSecretKeystoreTest. UI tests write phone light/dark screenshots into the app's external files/parity-screenshots directory.

Fusion is absent from mobile runtime assembly, so navigation/search omit it and its legacy route explains Desktop availability without editable controls. Final native verification must use the rebuilt engine with settings/admin command handlers and matching bindings. Canonical localization generation and --check pass for all five locales; git diff --check passes for the settings scope. Root owns final device/visual verification and generated-runtime integration.

## Final cleanup and import extension

Replace the legacy main-list UI test with the production desktop navigation, then remove its unused mock main, Dream, MCP and simple account/privacy/input/notification implementations. Keep legacy routes and persisted models; routes already point at live native surfaces. Add bulk provider paste/file import with a sanitized review, explicit conflict selection, credential acknowledgements, captured-layer checks and unit/UI regression coverage.
