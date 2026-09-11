# Native mobile Desktop parity

Baseline: Desktop commit `e0d900b8fb3033be23718a22694db92a9ee561f5`.
Android remains Compose; iOS remains SwiftUI. No new third-party dependencies, WebView transcript,
credential migration, session reset, or manual edits to generated bindings.

## Implementation and cleanup sequence

1. Preserve existing conversation/reducer/settings/project tests; add regressions
   before replacing presentation rules.
2. Native chat lanes align messages, grouped tools (last-tool summary), active-only
   Thinking, stable expansion, composer controls and runtime details.
3. Native settings lanes align four groups/search and connect supported engine
   snapshot/update/admin APIs, preserving layers, managed read-only, native extras.
4. Root supplies original paired avatars, identical UTF-16 ID hash, capability
   integration and application-level wiring. Project lane verifies first-message
   catalog visibility and selection without keyboard focus stealing.
5. Integrate and compile both platform variants; run native unit/UI tests and
   screenshot comparisons at phone/tablet widths and both themes. Remove old
   presentation paths only after replacements are wired and tested.

## Acceptance matrix

| Behavior | Desktop reference | Android | iOS |
|---|---|---|---|
| Messages/Markdown/code/images | Stage, MarkdownContent | ChatScreen/MessageBubble | ConversationTimelineView/MessageBubble |
| Tool grouping/last tool summary | transcriptRows, ToolGroup | conversation projection | timeline segments |
| Thinking/live status | transcriptRows | execution projection | execution projection |
| Agent identities/details | AgentAvatar, RuntimeCenter | native avatar + execution detail | native avatar + execution detail |
| Composer/model/permissions | BetaComposer | native composer/controls | native composer/controls |
| First-message session visibility | submittedSessionCatalog | ProjectStore | ProjectStore |
| Settings groups/search/pages | settings/nav.ts | settings host/repository | settings host/repository |
| Layered settings/admin | useEngineSettings/admin APIs | existing UniFFI commands | existing UniFFI commands |
| Adaptive layout | desktop panes | phone + 840dp split | phone + 840pt split |

## Required checks

- Shared identity vectors and event scenarios: running/settled/error tools,
  reasoning between tools, narration boundaries, queued sends, session switch,
  stale catalog, archive filtering, permission races and background recovery.
- Settings read/write/round-trip, layers, managed values, failed writes preserving
  draft and project switch invalidating stale responses.
- Android Direct/Play build + JVM + Compose instrumentation; iOS Store/Full build
  + unit/UI tests. Rust tests for any changed binding/engine implementation.
- Visual checks at phone/tablet, dark/light, large text and reduced motion.
- No completion claim for placeholder-only pages or unconnected controls.

## Engine integration decision

Mobile engine assembly now reuses the existing internal `core` settings loader.
The added workspace crate edge preserves canonical user/project/local provider and
routing precedence instead of maintaining a mobile-only merge implementation.
Explicit provider allowlists remain authoritative; legacy launch configuration is
a fallback for values absent from file-backed settings. Generated bindings and
native libraries must be replaced as a matching pair after protocol generation.

Plugin secret forms reuse the existing platform secure-storage adapter and Rust
secret envelope. Save/delete never exposes stored values. An explicit guarded
reconnect refreshes the engine secret cache after active work has finished.

Actual native engine round-trip verification exposed that the mobile protocol had
settings/admin DTOs but the mobile host did not dispatch their commands. The
storage/admin implementation is therefore extracted from bridge-server into the
internal `configuration-admin` crate, with bridge-server reexports preserving its
API and behavior. This adds internal workspace edges, not new third-party
packages or transport protocols. Mobile dispatch uses the connection event sink
so standalone acknowledgement/errors cannot be discarded by live-turn filtering.

MCP legacy mobile entries are copied once from the old shared settings file to
app-private `mcp-config.json`. The original file is retained. Canonical atomic
config writes and a migration marker prevent concurrent-start corruption and
reimporting a server after the user removes it.
