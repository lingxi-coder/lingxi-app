---
name: template-selection
description: Choose the simplest available Local App template from the Host semantic catalog and submit it for validation.
---

# Select a Local App template

Read `LocalAppTemplateCatalog` before choosing. It returns the Host's current
catalog digest and only available semantic entries: `templateId`, `surface`,
`summary`, `recommendedFor`, `notFor`, `mcpDefaultEnabled`, and
`mcpSuggestions`. Family, revision, paths and hashes are deliberately
withheld and must never be inferred. The last two fields are informational
context for a LATER per-App MCP-authoring step; a create run never acts on
them — do not enable MCP, reference `mcpSuggestions`, or let either field
leak into your `reason` text. Babylon stays out of the production catalog
while its real-device availability gate is pending.

Choose the simplest available entry matching the confirmed requirements.
Prefer the catalog's `dom` template entry for ordinary forms, lists, data and
navigation; choose canvas or an engine only when the brief requires drawing,
3D, scenes, sprites, tilemaps, collision or a game lifecycle. Read the
`templateId` the catalog actually returns rather than assuming a fixed
revision suffix — it changes as the catalog is revised.

Record at least one rejected candidate for the display-only confirmation UI.
Nothing enforces that minimum: an omitted, empty or non-array `rejected` is
accepted silently, so this is a rule you keep, not one the Host checks. What
the Host does check is shape and membership — every entry needs a non-empty
`template_id` and a non-empty `reason` (truncated to 1000 chars) or the whole
call is refused, only the first 16 entries survive, and any entry whose
`template_id` is not in the catalog you just read is dropped without a word,
so it silently never reaches the confirmation UI. Copy `templateId` values
verbatim from `LocalAppTemplateCatalog`; never invent or remember one. If the
catalog is later revised out from under the run, a journaled rejected
candidate that left it makes the downstream
`LocalAppResolveTemplateSelection` refuse with
`validated_selection_invalid: rejected candidates escaped the current
catalog` — a permanent, restart-the-run refusal.

Submit the proposal to `LocalAppValidateTemplateSelection` with the exact
`catalog_digest`, `template_id`, `reason`, `rejected`, Host-supplied `app_id`
and `workflow_run_id`, plus the Host-issued `selector_capability`. Never send
`caller_role`; the Host proves selector identity from the capability, not from
model text. Return the Host-issued `validated_selection_handle` unchanged. The Host re-reads the
catalog and rejects stale, unavailable, forged, cross-app or cross-run
handles; downstream agents resolve the handle through
`LocalAppResolveTemplateSelection`.

Do not call `LocalAppScaffold`, write source, edit package manifests, or
provide profile identity/path/hash fields yourself.
