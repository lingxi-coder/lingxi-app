# PR09: trusted evidence integration implementation handoff

Status: implemented and integrated on `codex/fusion-optimization`; affected unit suites and the two real-pool evidence end-to-end tests have passed. Combined PR09–PR11 verification is recorded in `2026-09-07-fusion-final-verification.md`, including baseline/environment limits. Refines the approved `2026-09-06-fusion-optimization.md` and `.omx/plans/test-spec-fusion-optimization-2026-09-06.md`; it does not reopen their decisions. No new dependency, paid request, extra judge call, or ordinary Agent behavior change is authorized.

## Implementation and verification checkpoint

- The observed registry result now reaches the actual runner, selected history, provider adapter, canonical request and HTTP/SSE codec proof. Inclusion is upgraded only at authorized physical dispatch; producer drain precedes freezing.
- Report-local attestations follow anonymous panel reassignment and both judge packing paths. The synthesizer validates `[evidence:evr_<32 lower-case hex digits>]` against references in its final prepared payload. Invalid citations retain charged usage and produce `NeedsParent`, without another model call; absence of citations is not positive verification.
- The real-pool tests exercise `RegistryToolInvoker` → `PoolSubagentSpawner`/runner → `ProviderApiAdapter`/`ApiService` → fake transport → analyst/synthesizer. Valid references, forged references with preserved usage, and legacy evidence-less results pass. Provider IO and tool data come from fixtures; capture/delivery markers are not manually substituted. These use legacy accounting; durable accounting is covered by separate host tests.
- Verified affected suites include platform-api 355, tool-api 200, agent 433, llm-client 869, Fusion 278, and the two Desktop evidence end-to-end tests. These counts describe the tested snapshots, not a claim that the entire workspace release gate is green.
- Explicit coverage limit: custom raw `body_bytes` and nonempty UTF-16 body overrides do not receive a structural proof and remain `Fetched`. Evidence-bearing legacy calls use the observed HTTP path; no WebSocket inclusion claim is made. Unsupported mappings fail closed rather than deriving authority from matching strings.

The sections below preserve the implementation contract and its historical starting point; future-tense patch instructions are design history, not remaining feature work.

## Scope and historical starting point

Reuse `platform-api/src/evidence.rs`: host-minted receipt/block identities; isolated run/panel stores; streaming full-result digest; UTF-8-safe retained prefixes; caps of 512 KiB per receipt, 2 MiB per panel, 16 MiB per run; source/cache timestamp/status; metadata retained when body capacity is exhausted; freeze. `fusion/src/evidence.rs` already wraps run/panel construction. Do not implement a second store.

`tool-api/src/tool_invoker_impl.rs` captures after permission and successful `Tool::call`, while `is_error` is available. Concrete `EvidenceCapability`, not a displayed name, selects the reviewed extractor. It currently discards the returned receipt, and the evidence builder is used only by tests. `included_in_request` is never set. Fusion has no production scope installation, receipt DTO field, delivery observer, or merged citation validator.

The runner's success arm currently converts Value into media/text and creates `ContentBlock::ToolResult`, then constructs a fresh `ConversationMessage::User`. `cap_input_bytes` selects history before provider submission. `ProviderApiAdapter` passes opts into `ApiService`; the physical HTTP/SSE loops prepare the request, authorize the model attempt, call `mark_dispatched`, then invoke transport. These are separate trust boundaries, not interchangeable observation points.

## Trust contract

Fetched means an authorized reviewed tool successfully produced the captured value. IncludedInRequest means that exact captured result, bound through host-owned provenance to its real result block, was included in an actual submitted model request. It does not prove server acceptance, billing, current file contents, or the truth of the model's interpretation.

Neither a model-authored `tool_use_id`, matching digest, matching content, receipt string copied into a message, nor `verified: true` confers authority. Equality is an integrity check after possession of a scoped host capability, never a substitute for the capability. Raw bodies and private run/panel UUIDs never enter telemetry, task summaries, durable snapshots, or sibling panel contexts.

The opaque public receipt reference is a citation handle, not a secret authorization token. It may be shown to the originating panel and then in that report's judge payload. The underlying run/panel UUIDs, block token, and store handles are never serialized.

## Interface sketch and true owners

Names below are proposed Rust APIs, not existing interfaces. Freeze their semantics before splitting implementation. Constructors must validate panel scope; fields granting authority remain private; Debug is redacted; serialized forms cannot recreate capabilities.

```rust
// platform-api owns these cross-crate capability types.
pub struct ToolInvocationResult {
    pub value: serde_json::Value,
    pub evidence: Option<CapturedToolEvidence>,
}
pub struct CapturedToolEvidence { /* private receipt + panel owner */ }
pub struct EvidenceHistoryBinding { /* private owner, block token, identity, integrity */ }
pub struct EvidenceDelivery { /* immutable Arc-backed selected bindings */ }
pub struct EvidenceAttestation { /* sanitized immutable host projection */ }
```

- Add `ToolInvoker::invoke_observed` and a workspace-lease equivalent, returning `ToolInvocationResult`. Defaults invoke the existing corresponding method and return no evidence. Registry overrides use one shared implementation for permission/execution/capture, not duplicated gates. Existing `invoke` paths return only the value and remain behavior-compatible.
- `CapturedToolEvidence::bind_result_block(...)` consumes the capture and returns an `EvidenceHistoryBinding` only when supplied the expected successful runner-visible result content. Bind to the newly created host `MessageId` plus block ordinal and the opaque capture token. The digest/length of the canonical full result checks integrity. Reject media/summary transformations not supported by the reviewed evidence extractor rather than attesting to a different payload.
- `EvidenceContext` owns receipts and updates. `EvidenceDelivery` owns a fixed selection of capabilities, not a mutable shared current-turn list. Its construction validates ownership and selection; public methods cannot manufacture a token from strings.
- `SubagentSpawnRequest` gets a `#[serde(skip)]` optional evidence context. `AgentContext` receives it only for a trusted Fusion panel. `SubagentInvocationContext` carries the same optional trusted context into Registry; this avoids replacing/downcasting the inherited registry or violating registry Arc identity. A context supplied with an ordinary tool policy does not enable capture. Existing registry builder can remain for isolated tests, but production uses the explicit per-invocation owner; never install a shared mutable global current panel.
- `SubagentApiCallOpts` owns `Option<EvidenceDelivery>` for one call. The production adapter forwards it to `LlmRequest` as a `#[serde(skip)]` optional field. Equality/clone behavior must remain compatible with existing derives without making authority serializable. Do not put per-turn delivery into the shared `ModelAttemptContext`.
- `llm-client` owns the codec/prepared-request proof and physical marking operation. A private prepared proof binds a delivery selection to the exact encoded request body and supported serialization mapping. Platform store APIs accept only host capability-based marking; model JSON is never a marking API.

Prefer wrappers/additive option structs to another long positional service method parameter list. Old helpers delegate with no evidence. No new trait object in a serialized DTO.

## Block binding from capture to actual transport

1. Registry obtains the post-permission tool result, validates capability and `!is_error`, captures the existing bounded material, and returns an opaque capture beside unchanged Value. Errors return no capture. Lease variants use the same path.
2. Runner allocates the tool-results message ID before constructing its successful result blocks. It derives text/media exactly as today and binds the capture to `(message ID, block ordinal, opaque block token, canonical content integrity)`. Keep bindings in a runner-local map, outside serialized history. Duplicate tool IDs produce different bindings. Do not change the protocol-wide ContentBlock schema merely to store host authority.
3. Provide the panel its host-created citation reference in a separately defined, trusted suffix to this tool result, with clear boundaries. Preserve original captured content and its digest separately from this suffix. Bind the final model-visible block as well as the captured material; a copy of the suffix is still not a binding. The evidence-less path must be byte-identical. Update byte accounting using the final text including the suffix.
4. After `cap_input_bytes`, inspect the actual selected messages. A binding is eligible only when the exact host message ID, block ordinal, result variant and content integrity still match. Missing/replaced/compacted blocks have no eligible binding. Discard obsolete runner bindings when their history messages are permanently removed. Selection is immutable for this API call, including retries.
5. The adapter maps eligible selected positions into canonical `LlmRequest.messages` positions while performing its normal conversion. Preserve provenance through this trusted conversion explicitly; do not recover it by searching strings afterward. If conversion drops, combines, or transforms a bound result without a reviewed exact mapping, omit its proof and keep it Fetched.
6. During codec preparation, preserve a correspondence between each proven canonical result block and the structural output location actually encoded for the selected protocol. The private prepared proof contains those bindings plus a digest of the exact encoded body. Checking body equality alone is not authority: proof was created from the owned bindings and the encoder's structural mapping. Arbitrary whole-body substring searches cannot implement this step.
7. After request preparation and successful attempt authorization/live-policy check, immediately adjacent to the physical transport call with no intervening await, validate the proof against the final body and mark only its included receipts. Request-header injection does not change body proof. Body mutations require re-preparation; never use stale proof after fallback/retry rewriting.

Initially implement and test the protocols used by the existing Fusion HTTP/SSE path. Unsupported encoder transformations retain Fetched/unverified and must not fabricate inclusion. Do not enable an unsupported Fusion WebSocket path or claim all protocols covered. If any production-supported Fusion protocol lacks proof, report the coverage gap rather than silently declare PR09 complete.

## Lifecycle, failures, and retries

| Event | Evidence behavior |
| --- | --- |
| Permission denial, tool error, logical error, invalid HTTP/redirect result | No receipt |
| Successful capture, no next model call | Fetched only |
| History cap removes captured block | Fetched only for that call |
| Block survives a later call | Included only at that later dispatch |
| Prepare/admission/budget/WAL/live-policy failure before dispatch | No new inclusion |
| Transport submission fails or response is canceled | Inclusion remains; it is not remote receipt/acceptance |
| Retry with unchanged body | Revalidate each physical prepared request; marking is idempotent |
| Retry/fallback changes canonical body | Rebuild structural proof; removed content is not marked by old proof |
| Same text or tool ID appears in another block/panel/run | No authority without that exact binding |
| Panel ends, panics, quorum cancels, or run cancels | Drain actual producer, then freeze capture and delivery; retain existing receipts |

Freeze must happen after physical producers drain, not merely after a cancellation acknowledgement or panel wrapper completion. Keep the run's store owner reachable from the run barrier so error/panic paths can freeze all panels. A late untrusted marking request after freeze cannot upgrade status. Registered accounting remains independently settled; an evidence failure cannot drop known usage or trigger another model call.

Current `EvidenceReceipt` compares full snapshots for `body` and `owns`; updating `included_in_request` can stale old snapshots. Resolve latest receipt at read time or change authorized body lookup to use immutable receipt identity plus scope. Do not weaken it to digest-only equality. Capture immutable metadata separately from monotonic delivery state to avoid accidental revocation of valid capabilities.

## Reports, judge packing, and merge validation

Add optional `PanelEvidence.receipt_ref: Option<String>` with default/skip-none serialization. Extend panel schema, prompt and sanitization; update Rust literals. Old reports remain readable and unverified, not fabricated as verified and not categorically rejected.

After drain/freeze, resolve each supplied reference only in that panel's store. Produce a separate host attestation projection keyed by report-local evidence ID and receipt reference. Include provenance/delivery, full digest, retained/full byte lengths, truncation and cache timing without private scope identifiers. Model-authored fields never overwrite host projections. Check kind and locator digest; attest excerpt support only when present in retained material under a defined exact normalization. An unavailable truncated tail is unknown, not disproved. For files, the digest attests the captured result at read time, not the current file state.

`PanelInternal` or a parallel host-owned per-panel map retains the frozen evidence view. Anonymous panel reassignment must move the associated view with its report, never re-key by provider/name. No sibling receives another panel's context/body store. Judge payloads may contain bounded report-local cited excerpts and attestations after panel isolation has ended.

Update both full and packed analyst/synthesis paths. Existing arbitrary `claims_evidence_excerpt` string truncation is insufficient for verified references: emit structured cited metadata as mandatory when a surviving claim references it, or explicitly omit the claim/reference with counts. If mandatory cited metadata cannot fit, return the existing no-judge-call packing failure/NeedsParent path. Preserve byte-identical full payload for evidence-free legacy reports where practical.

Choose one unambiguous merge citation syntax, e.g. `[evidence:<opaque-ref>]`, documented in the synthesis instruction; do not scan arbitrary URLs as authoritative receipts. Freeze the set of validated references actually included in the final prepared synthesis payload, not every receipt ever captured. Require returned citation handles to be a subset of that set. Invalid/forged/omitted/cross-panel handles produce NeedsParent with a deterministic citation-integrity reason and useful panel fallback, no second synthesis or verification LLM. Preserve successful synthesis usage even when the answer is rejected. Host must not label an uncited answer as evidence-verified merely because it contains no invalid handles; any required citation coverage rule must be explicit and tested rather than inferred from empty-set subset success.

## Two implementation lanes and frozen interface boundary

At most two source writers. Do not overlap PR08 source ownership. Root alone executes Cargo sequentially against the shared target.

**Lane A — capability and transport chain** owns platform evidence/tool-invoker/spawn types and exports; tool-api registry capture return path; agent context/handle/runner/API; orchestrator provider adapter; llm-client request/service/codec proof. Its first handoff freezes type signatures, invocation defaults, block binding, per-call delivery, and frozen receipt access semantics. Lane A does not edit platform Fusion DTOs or Fusion orchestration.

**Lane B — Fusion consumption** owns platform-api `fusion.rs` DTO additions (explicit file handoff), Fusion evidence/panel/packing/analyst/synthesizer/orchestrator and their tests. It consumes Lane A's frozen APIs, supplies trusted contexts at spawn, and implements report-local attestations and merge validation. It must not independently revise shared capability contracts. While Lane A finalizes interfaces, Lane B can prepare evidence DTO/schema/packing tests, not assume a fake delivered flag satisfies end-to-end coverage.

Independent review may run read-only alongside both. Stage source changes before Cargo starts, freeze interfaces for compilation, then run one coordinated affected package union. Do not run two shared-target Cargo jobs to simulate parallel throughput.

## Regression gates

- Keep all existing cap/digest/cache/freeze and permission/workspace-lease tests.
- Add actual registry→runner result→history selection→adapter→supported codec→fake transport closed-loop tests. Do not replace this with manually setting Included.
- Duplicate tool-use IDs, copied suffixes/receipt strings, identical content in sibling messages, spoofed model verified fields, wrong run/panel, altered block under same ID, serde round trip losing capability.
- Fetched without next call; trimmed then later included; preparation/budget denial; failed transport; retry body changes; cancellation before/after marker; frozen store rejects upgrades; stale receipt snapshots remain safely readable by identity.
- All three byte limits, metadata after zero retained bytes, multibyte suffix byte accounting, truncated excerpts, file changes after capture, cache hit timestamp/hash.
- Full/packed judge payload consistency, mandatory referenced metadata exhaustion causes no network, omitted claim counts, anonymous mapping continuity, no provider/private UUID/store leakage.
- Valid merged handle, forged/cross-panel/packed-out handle, legacy no-receipt report, known usage preserved on invalid merge, no extra LLM call. Verify that no-citation output is not represented as positively verified.
- Ordinary Agent results and existing ToolInvoker implementations stay behavior-compatible; unsupported delivery opts cannot silently claim evidence was included.

## Minimal first patch and residual risks

First patch: additive observed invoker result, registry capture handoff, and runner-local opaque binding with RED/GREEN forged/reused-ID tests. Keep production evidence opt-in disabled and all receipts Fetched. This establishes the trust boundary without changing judge decisions or claiming delivery. Next patch adds immutable per-call selection and actual transport proof; only then enable production scope plumbing and consuming attestations.

Highest risk is canonical-to-provider transformation: a blanket prepared-body hash or tool-ID search is not sufficient. A second risk is freeze ordering across wrapper/physical producer lifetimes. A third is packing silently cutting the structured reference authority. These require independent review before production enabling. No live-provider evidence, source changes, Cargo runs, or performance claims were produced while preparing this document.
