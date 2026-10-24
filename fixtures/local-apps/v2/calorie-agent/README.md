# Mobile Agent schema-2 acceptance fixture

Uses the host-owned `localApp.agent` task contract and native camera/photo picker. No provider secrets or tool grants are stored in the application. Authorize a configured vision/tool-capable model from the native application details page before running.

The application compresses images before submission, validates structured results in addition to the host schema validation, lets the user edit portions, then saves only after confirmation. Model estimates are labelled as estimates. It requests no business tools.

This fixture is derived from the accepted Desktop calorie example. It is source code for physical acceptance, not evidence of completed device tests. The bundled image used during acceptance is separately attributed in the acceptance directory.
