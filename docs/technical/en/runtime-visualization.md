# Inline visualization integration

Publish immutable visualization revisions and mount them through one shared authorized service on Electron, Android and iOS.

<a id="host-opt-in"></a>

## Capability and host opt-in

Desktop/mobile compositions opt in with inline_visualization only when the host WebView can render the widgets. The flag registers the Visualization tool and the visualize skill. Publish stores an immutable revision in the current conversation; list returns those revisions and does not create a public URL. CLI and headless leave the flag off, so none of this is advertised. A browser-like rendering surface is an explicit prerequisite.

The runtime shares one store per config-home in the process so the publishing tool and host service serialize writers consistently. Hosts forward scheme requests and shell messages; the shared service owns routing, authorization, document assembly, CSP and state comparison-and-swap.

<a id="revision-storage"></a>

## Immutable revisions and confirmed state

Storage is rooted at <config-home>/visualizations/<root-session-uuid>, with index.json, immutable revisions/<id>/<rev>.html and versioned state/<id>/<rev>.json. Writes use rooted atomic write/sync/rename and are acknowledged only after success. Advisory locking serializes writers across processes.

Confirmed widget state keeps modelContent and privateContent separate. The combined state limit is 16 KiB; session fragment storage is limited to 64 MiB and each visualization ID to 50 revisions. Validation/quota/storage failures return explicit errors before the UI may claim a saved state.

<a id="mount-lifecycle"></a>

## Authorized mount lifecycle

1. Ask VisualizationService to mount a root-session revision with theme, locale and expansion state. It returns a capability token, generation and document URL.
2. Forward the WebView scheme request to the shared service and preserve its status/CSP headers. Do not bypass the service by reading arbitrary fragment files from the UI.
3. Relay state writes with the live mount identity and expected version. A stale mount or conflicting version is rejected; use returned winning state on conflict.
4. Unmount on replacement or session transition. Session deletion and orphan sweep clean storage through the owning runtime rather than arbitrary UI filesystem deletion.

<a id="followup-context"></a>

## Follow-up context is snapshotted at send

A SendPrompt may carry visualization_context {id, revision}. The runtime reads that revision’s confirmed modelContent when the prompt is sent/enqueued and fences it ahead of the typed text. Unknown session/id/revision leaves the original text unchanged. Widget privateContent is not promoted into model context.

### ClientCommand · follow up on revision 2

```json
{
  "type": "send_prompt",
  "text": "Explain the selected option",
  "images": [],
  "visualization_context": {
    "id": "viz-example",
    "revision": 2
  }
}
```

Verification method: API excerpt

Replace the example ID with an ID actually returned by publication. The field does not authorize reading a different conversation’s visualization, and an invented ID simply contributes no follow-up context.

<a id="private-pages"></a>

## Private pages stay on this computer

A private page is a different flag, private_pages. Electron turns it on and registers the Page tool. CLI, headless, and mobile leave it off. The page is stored at <config-home>/pages and opened inside LingXi. There is no public URL.

Publish stores an HTML fragment. The document shell, including the offline content security policy, is applied when the page is served. list returns id, revision, and title for pages on this computer. A reference line places the page in the transcript. The session bridge serves that revision. Nothing is uploaded.

<a id="server-pages"></a>

## A static page can be published to the LingXi server

A static Local App whose entry file is a complete HTML document, and which declares no install, build, or process, can be published from the desktop market action after sign-in. The review exports the declared source files. Publishing that app into the device Apps library does not upload it.

When that release is published and the listing is available, the market returns a preview address of the form https://v-<version id>.lingxios.net/<entry path>. The server serves the package file at that address. An unknown version is unavailable. A package with a server entry, install, build, or process is distributed as source, and the server does not run it.

<a id="visualization-acceptance"></a>

## Cross-platform acceptance

- Tests exercise revision storage, token/generation authorization, stale state rejection, quotas and shared host behavior. Verify theme/locale, collapsed and expanded mounts and follow-up context against copied prompts.
- Electron custom scheme and iOS/Android WebView message routing, touch behavior, accessibility and CSP enforcement still need actual host UI acceptance. A shared Rust test does not prove all three native viewers.

### Shared store and host contracts

```bash
cargo test --locked -p visualization --lib
cargo test --locked -p harness-runtime --features desktop --test inline_visualization
cargo test --locked -p harness-runtime --features desktop,mobile --test visualization_host
```

Verification method: Environment validation

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/inline_visualization.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb7a016c305609a0b4ca7b0c1a91faead18c4db0/crates/runtime/src/inline_visualization.rs) — Committed source (bb7a016c305609a0b4ca7b0c1a91faead18c4db0)
- [harness-runtime/crates/runtime/src/desktop_visualization.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb7a016c305609a0b4ca7b0c1a91faead18c4db0/crates/runtime/src/desktop_visualization.rs) — Committed source (bb7a016c305609a0b4ca7b0c1a91faead18c4db0)
- [harness-runtime/crates/runtime/src/mobile/visualization_host.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb7a016c305609a0b4ca7b0c1a91faead18c4db0/crates/runtime/src/mobile/visualization_host.rs) — Committed source (bb7a016c305609a0b4ca7b0c1a91faead18c4db0)
- [harness-runtime/crates/tools/ui/src/inline_visualization.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb7a016c305609a0b4ca7b0c1a91faead18c4db0/crates/tools/ui/src/inline_visualization.rs) — Committed source (bb7a016c305609a0b4ca7b0c1a91faead18c4db0)
- [harness-runtime/crates/visualization/src/store.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb7a016c305609a0b4ca7b0c1a91faead18c4db0/crates/visualization/src/store.rs) — Committed source (bb7a016c305609a0b4ca7b0c1a91faead18c4db0)
- [harness-runtime/crates/visualization/src/service.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb7a016c305609a0b4ca7b0c1a91faead18c4db0/crates/visualization/src/service.rs) — Committed source (bb7a016c305609a0b4ca7b0c1a91faead18c4db0)
- [harness-runtime/crates/visualization/src/document.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb7a016c305609a0b4ca7b0c1a91faead18c4db0/crates/visualization/src/document.rs) — Committed source (bb7a016c305609a0b4ca7b0c1a91faead18c4db0)
- [harness-runtime/crates/visualization/src/context.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb7a016c305609a0b4ca7b0c1a91faead18c4db0/crates/visualization/src/context.rs) — Committed source (bb7a016c305609a0b4ca7b0c1a91faead18c4db0)
- [harness-runtime/crates/visualization/src/checks.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb7a016c305609a0b4ca7b0c1a91faead18c4db0/crates/visualization/src/checks.rs) — Committed source (bb7a016c305609a0b4ca7b0c1a91faead18c4db0)
- [harness-runtime/crates/runtime/tests/inline_visualization.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb7a016c305609a0b4ca7b0c1a91faead18c4db0/crates/runtime/tests/inline_visualization.rs) — Committed source (bb7a016c305609a0b4ca7b0c1a91faead18c4db0)
- [harness-runtime/crates/runtime/tests/mobile/visualization_host.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb7a016c305609a0b4ca7b0c1a91faead18c4db0/crates/runtime/tests/mobile/visualization_host.rs) — Committed source (bb7a016c305609a0b4ca7b0c1a91faead18c4db0)

- harness-runtime: HEAD bb7a016c305609a0b4ca7b0c1a91faead18c4db0; Referenced input fingerprint sha256:700726d3f012384673f588a15ab3a32ecaf4f3fc499f0e4e86c0b8e6bc321559; Product pin ae5a65b1f4816cd26a06d462573ebc9841f14f96

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
