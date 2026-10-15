# Product Desktop render-site inventory for Native 2.1.289

This note separates the Native `ui_render` schema's admitted inputs from sites
the Native CLI actually produces and from Product Desktop's real row producers.
It records the current Product integration boundary; it does not claim that a
React component, helper test, or schema admission is a Native CLI callsite.

The pinned Claude Code macOS arm64 executable SHA-256 is
`03d66745e3bb69ec727d66023696f3820bc0a00a8a5ba725eb6706d0c67cbe69`.
All byte ranges below are zero-based, absolute, half-open ranges in that
executable. The extracted `src_*.js` names identify the bundle source assets.

## Native admission and production evidence

The component enum contains exactly these 15 values, in source order:
`AskUserQuestion`, `UserMessage`, `AssistantMessage`, `ToolUse`, `ToolResult`,
`ToolGroup`, `ToolProgress`, `CommandOutput`, `Spinner`, `TurnDuration`,
`InfoNotice`, `SessionMode`, `PromptHint`, `AbovePrompt`, and `Pane`.
The pinned range is `src_180514638.js` bytes `181077075..181077472`, SHA-256
`9fb3958e6ae4fe5b1acb3c594e8858a119e7aec0c098c9fa4ef20946334ab85e`.

The remote Host `ui_render` control surface enum is `desktop | mobile | vscode`,
independent of the component enum. Its range is `src_180514638.js` bytes
`181086680..181086951`, SHA-256
`c55b457a61defe5437dba97804026333c4e47aeef09649aead405b615c747a09`. The
`ui_render` request parser validates `surface` and `component` separately and
has no cross-field refinement: `desktop + AbovePrompt` is admitted. The schema
range is `181089372..181092130`, SHA-256
`7c95088c971a840590228a77f3d578c378992f14589cda3b99820bf3131eda3f`.

The Mod hook event `ui.render` has a separate generic SDK dispatch which
accepts the event name without selecting a fixed component;
`src_182865546.js` bytes `182877103..182877173`, SHA-256
`280bf2a9e65a2b798c72cd12a479a7d1f3f359f43eb99895a68c8d3fd95d56a9`. This
proves the generic API can route an admitted `ui_render`; it does not prove the
CLI creates a request for every admitted component/surface pair.

The Native CLI TUI's own hook event request builder `jO` hardcodes
`surface: "terminal"` at
`src_203302718.js` bytes `203345326..203345627`, SHA-256
`2bfab65626a615fe036224f8375262845be461cac3946cbc3e7c4304c02e5d91`. The
observed TUI `AbovePrompt` producer is
`src_208361606.js` bytes `208791724..208791879`, SHA-256
`14240165745a4c4e822bbdfc0d80e1774a47b44e06333cd21d78a96461722eeb`. The
observed TUI `Pane` producer is the same asset's bytes
`208803810..208803991`, SHA-256
`b4368248dd7bf3409502fdc06b5983dda93aa7c821a67f1dc01d5cbbe6d14e05`.
Those requests use `terminal`; neither callsite establishes a Native CLI
Desktop producer. Product's Desktop `AbovePrompt` is therefore an external
Desktop consumer made possible by the generic admitted schema, not a claim
that the pinned CLI itself produces a Desktop `AbovePrompt` request.

Other Native TUI producer observations used for classification include:
`SessionMode` passes a `modes` list from the terminal footer; `PromptHint`
passes terminal draft/working hint state; `TurnDuration` uses a duration word
and milliseconds; `ToolUse`, `ToolResult`, `ToolGroup`, and `CommandOutput`
are created from transcript rows. Their complete producer byte windows and
individual range hashes were not captured in this Product note, so they are
not claimed as byte-golden assertions here. The executable SHA above remains
the reproducible source identity.

## Product Desktop producers

Product's current Desktop row state has a direct producer for these seven
Native transcript sites. The hook tree replaces only the corresponding row;
when the host has no render tree or the tree yields an `engine` node, Product
keeps the existing engine row as fallback.

| Native site | Product-owned source | Product facts sent |
| --- | --- | --- |
| `AskUserQuestion` | `apps/electron/src/renderer/App.tsx`, pending typed question request | Actual question/options data and the request's stable ID. Existing submit/cancel callbacks remain the engine fallback. |
| `UserMessage` | `Stage.tsx`, narration with `role: user` | Text. The engine fallback retains Product's attached image rendering; image data URLs are not copied into the hook props. |
| `AssistantMessage` | `Stage.tsx`, narration with `role: assistant` | Text plus first-of-reply derived from the visible transcript order. Plan-preview rows stay on their existing engine presentation path. |
| `ToolUse` | `ToolCall.tsx`, real tool-use row | Tool-use ID, name, parsed original input, status, and output when the reducer received one. |
| `ToolResult` | `ToolCall.tsx`, completed result slot | Tool-use ID, name, parsed original output, and error state. The built-in result display continues to use the engine-derived display DTO. Product keeps the completed result site mounted in the collapsed slot while passing an empty engine fallback; opening the disclosure reveals that same result tree once. Tool calls without an expandable body still create the completed result site with an empty default slot, so hooks do not depend on disclosure state. |
| `ToolGroup` | `Stage.tsx`, an actual multi-tool transcript group | The grouped tool calls, active state, and the Product disclosure state. Single-call wrappers do not invent a `ToolGroup` site. |
| `CommandOutput` | `Stage.tsx`, a slash-command result row | The command, retained arguments when available, output text, and error state. |

The `tool_use`/`tool_result` reducer preserves a parsed copy of the original
JSON payload solely for these UI props. Product's default cards still render
the pre-derived `ToolHeaderDto` and `ToolResultDisplayDto`; Native props are not
used to reconstruct Product's normal presentation.

## Remaining sites and boundaries

| Native site | Current Product status | Boundary |
| --- | --- | --- |
| `AbovePrompt` | External Desktop consumer exists in the Projection-owned generic parent-site runtime. | Schema-admitted for Desktop; the pinned CLI callsite found in this audit is TUI/terminal. Keep those claims separate. |
| `Pane` | No plugin pane producer or lifecycle. | Existing Runtime Center and terminal panels are Product host panels, not plugin panes. The Native TUI producer is terminal. |
| `ToolProgress` | No separate desktop row producer. | A running `ToolUse` row is not promoted to a different Native component. |
| `Spinner` | No separate desktop row producer. | Product thinking/retry indicators are existing engine status UI, not an asserted Native Spinner row. |
| `TurnDuration` | No exact site props retained. | Product has a formatted turn footer, but not the Native duration-word/milliseconds producer contract. |
| `InfoNotice` | No direct site producer. | `system_notice`, toast, and status lines remain their distinct Product channels; they are not relabeled as `InfoNotice`. |
| `SessionMode` | No compatible Product producer. | Native TUI's `modes` are footer status modes, not Product's permission mode picker or `chat | code` session mode. |
| `PromptHint` | No direct Native hint producer. | The Product composer placeholder is not used as a substitute for Native draft/working hint state. |

The `ui.log`, `ui.toast`, and `ui.status` channels are separate from `ui_render`
sites. `ui_client_press` is also a Client-surface control and is not a
`ToolUse`/`ToolResult` producer.

## Focused evidence in Product

`apps/electron/test/mod-ui-site-props.test.ts` checks the constructed site
props and response-props fallback behavior; `apps/electron/test/conversation.test.ts`
checks that reducer state preserves original tool input/output without
changing the engine-derived display. These are payload and reducer tests, not
Electron DOM or live Native acceptance. The transcript markup test also checks
that a completed `ToolResult` site remains in the collapsed row, its default
result body is not duplicated or exposed while closed, and the same body appears
once after expansion. This preserves Product's disclosure layout while the
Native `ToolResult` hook lifecycle is independent of whether the user opens it.
