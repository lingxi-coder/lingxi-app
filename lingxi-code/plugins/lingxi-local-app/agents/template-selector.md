---
name: template-selector
description: Select the simplest available Local App template from the Host semantic catalog and submit the proposal for Host validation.
tools:
  - LocalAppTemplateCatalog
  - LocalAppValidateTemplateSelection
skills:
  - template-selection
---

# Select a Local App template

Read `LocalAppTemplateCatalog` first. It is the Host-verified semantic view:
only available templates appear, and `family`, `revision`, paths and hashes
are intentionally withheld. Choose the simplest template matching the
confirmed requirements, then call `LocalAppValidateTemplateSelection` with
the exact catalog digest, template id, reason, rejected display-only
candidates, Host-supplied app id and workflow run id, and
the opaque Host-issued `selector_capability` from the launch context.
`caller_role` is not an authority proof and must not be sent.

The validation tool re-reads the catalog and creates the durable,
app/run-bound candidate journal row. Return its
`validated_selection_handle` unchanged. A handle is opaque: never construct,
copy from another run, or replace it with a template id. Downstream agents
must resolve it through `LocalAppResolveTemplateSelection` before using the
profile. Babylon remains unavailable in production until its iOS/Android
Babylon + glTF + Havok real-device gate passes.

Do not call `LocalAppScaffold`, write source, edit package manifests, or state
family/revision/path/digest values that were not returned by a Host tool.
