# Android conversation Desktop parity

Keep engine-derived tool display, durable transcript/recovery, pagination and scroll ownership intact. Existing reducer, tool retention and Markdown parser tests protect these boundaries. Add grouping tests before changing rendering: adjacent tools retain ordering, prose breaks groups, active tools alone appear while running, and the last tool supplies the settled summary.

Use native Compose groups and existing ViewModel disclosure IDs. Hide completed reasoning, retain only active thinking. Reuse shared agent artwork. Improve code copy and horizontal overflow and image wrapping. Keep current catalog search/provider grouping/recents and existing permission semantics. Verification: DirectDebug unit tests, Kotlin compile and Android lint; root owns device visual verification and cross-platform integration.

Follow-up: transcript rendering now extracts stable tool rows across assistant message envelopes, with prose/user/terminal-run boundaries covered by tests. Tables and nested emphasis have parser regression fixtures. ConversationDetailHost owns one selected detail independent of recycled rows; compact uses fullscreen and width >=840dp uses a 360dp side panel with live state resolution. No engine/session storage or pagination contracts changed.
