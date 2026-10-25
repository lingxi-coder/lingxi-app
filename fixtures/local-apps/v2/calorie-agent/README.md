# Mobile Agent schema-2 acceptance fixture

Uses the official AG-UI 1.0.3 `RunAgentInput`, standard BaseEvents, and `RunAgentResult` over the host-owned Agent task resource. Photos enter as inline AG-UI image parts created with `agent.file(File)`; the app stores only public run and thread IDs for reconnect. No provider secrets or tool grants are stored in the application. Select a model and grant any required permissions from the native application details page before running.

Mobile application pages are displayed in the external system browser, not an embedded WebView. Native host settings and approvals remain outside the page API. Browser site permissions are managed by the browser. On iOS, return to LingXi when the host needs foreground approval or wake; reopening the app reconnects read-only to its recorded task and never resubmits the meal.

The application compresses images before submission, validates structured results in addition to the host schema validation, lets the user edit portions, then saves only after confirmation. Model estimates are labelled as estimates. It requests no business tools.

This fixture is derived from the accepted Desktop calorie example. It is source code for physical acceptance, not evidence of completed device tests. The bundled image used during acceptance is separately attributed in the acceptance directory.
