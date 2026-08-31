---
name: template-selection
description: Choose the simplest available Local App template from the Host semantic catalog and submit it for validation.
---

# Select a Local App template

Read `LocalAppTemplateCatalog` before choosing. It returns the Host's current
catalog digest and only available semantic entries: `templateId`, `surface`,
`summary`, `recommendedFor`, and `notFor`. Family, revision, paths and hashes
are deliberately withheld and must never be inferred. Babylon stays out of
the production catalog while its real-device availability gate is pending.

Choose the simplest available entry matching the confirmed requirements.
Prefer `react-dom-r1` for ordinary forms, lists, data and navigation; choose
canvas or an engine only when the brief requires drawing, 3D, scenes, sprites,
tilemaps, collision or a game lifecycle. Record at least one rejected
candidate for the display-only confirmation UI.

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
