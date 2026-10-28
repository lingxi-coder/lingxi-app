# Local App hosting and remote access

Local Apps may run on Mobile or Desktop. Recommend Desktop for sustained work,
while preserving the user's Mobile deployment choice. Opening method and execution
host are independent: the default is the system's default browser, with Android's
explicit browser chooser and iOS's optional system in-app browser also available.
The connected computer's **Apps** section opens its shared applications without
installing their backend or copying their business data to the phone.

## Runtime and authorization

The existing immutable release and runtime remain authoritative on Desktop.
`LocalAppHost` adds explicit, trusted, per-app sharing settings and a loopback-only
HTTP gateway. A remote launch authorizes and starts the existing runtime headlessly;
existing native capability confirmation still runs on Desktop. Static releases and
server-rendered applications use the same gateway. Ordinary Desktop application
windows continue to work.

Remote startup has a 15-minute host deadline; viewer RPCs allow 15 minutes plus
30 seconds for the response. Expiry and access withdrawal abort pending work.
If a runtime activation commits after cancellation, its eventual settlement
still reconciles the unused run. Disabling independent background execution or
Agent access keeps an active foreground browser running, and a failed opening
releases a run retained solely for that opening.

The approved Remote Control connection exposes only `apps.list` and `apps.open`.
The catalog contains explicitly shared applications. The latter returns a
60-second, single-use bootstrap ticket in a URL fragment. Bootstrap removes the
fragment, exchanges the ticket for an HttpOnly host-only cookie, then navigates to
the application. Tickets, cookies and browser launches bind to the account,
approved device, Desktop host, application, release and runtime invocation.
An HTTP browser session lasts at most eight hours and survives an ordinary Remote
Control transport disconnect, including the phone switching to its browser.

Each opening gets a separate origin:

```text
https://<app-id>-<20-hex-routing-nonce>.<desktop-route>.apps.example.com/
```

The routing nonce is separate from the secret ticket and cookie. Different
openings cannot share browser authorization, Cookie state, localStorage or
IndexedDB, so an old tab cannot acquire a new run's credentials. Keep shared
business data in the selected runtime's backend rather than browser storage.
One wildcard certificate for `*.<desktop-route>.apps.example.com` covers these
single-label session hostnames; do not distribute a shared certificate private
key across unrelated Desktop hosts.

Account changes, Remote Control disablement, managed-policy withdrawal and device
revocation invalidate grants, including in-flight launch races. App stop,
permission withdrawal, sharing disablement and release changes invalidate its
affected grants. Streams and WebSockets close on revocation. Session-owned Agent
requests are cancelled when that session is revoked, even if another browser
still uses the same application run. Closing a browser tab alone does not signal
cancellation of durable tasks; task IDs and existing host task records remain the
recovery mechanism. Background execution still follows the application's host
grants and platform lifetime rules.
Application and session cancellation immediately abort active executors, with
terminal records settled through the ledger queue. Success checks cancellation
again when entering that queue. Chat reads share the mutation queue so pending
admission and rollback cannot expose provisional history. Mutations recheck
authorization at queue admission and immediately before dispatching their
filesystem commit; an OS operation already dispatched can finish. Once the last
session's Agent cancellation settles, the unused run is reconciled again. Idle
Local App RTC windows close before the process lifetime check.
Application permission withdrawal also invalidates pending mutations and
admissions while their browser session remains valid. Broker shutdown aborts
pending operations and active executors synchronously, and rejects new writes.
It waits for queued rollback and terminal persistence before returning; chat
creation shares that queue so its incomplete directory is cleaned before exit.

## HTTP, browser SDK and RTC

HTTPS handles page navigation, scripts, assets, backend requests, uploads,
downloads, WebSocket and SSE. It remains a complete transport when RTC is
unavailable. WebSocket service URLs use `/__services/<service-name>/...`; the
service must be registered for the same application run. The gateway never acts
as a general proxy to caller-selected ports or hosts.

Backend forwarding headers come from the validated browser session origin.
Absolute redirects to the registered service's loopback address are rewritten
to that origin, retaining a named-service prefix where applicable. Intentional
external redirects and other local ports are not proxied or rewritten.
Root-relative redirects from named services also retain their service prefix.
Their cookies use the corresponding `/__services/<name>` path, including a
mapped default path. A named service cannot set `__Host-` cookies, whose required
root path would break service scoping. Top-level application cookies retain
normal browser semantics.

The browser adapter provides `localApp.info`, the existing AG-UI Agent surface,
service URL helpers and browser file selection. Native process execution, native
file administration and permission approval are unavailable to remote pages.
Model/provider credentials stay on Desktop. Camera and file input use the chosen
browser's normal APIs. The host's Agent tool permission rules still apply.

RTC setup runs asynchronously after the SDK loads and reuses the existing hidden
Chromium peer infrastructure. At present, only bounded GET/HEAD fetches explicitly
using `credentials: 'omit'` are eligible. Cookie-authenticated reads, writes,
streams, ranges, downloads and large responses stay on HTTPS. The adapter keeps
the gateway's own authentication cookie while omitting backend service cookies,
matching the Desktop SDK's existing service credential convention. It also
suppresses backend Set-Cookie for logically omitted requests.
Eligible HEAD responses preserve the same reliable length metadata as HTTPS;
the resource handler removes metadata for injected or decompressed bodies.

Eligible RTC responses have a 256 KiB per-response and 1 MiB aggregate in-flight
budget, at most eight requests per peer and a 15-second deadline. Reads may fall
back to HTTPS. Writes never enter this path and are not replayed during transport
failure. The peer is scoped to the redeemed browser session and closes when that
session ends. Inbound RTC requests are limited to 64 KiB before reassembly.
Upstream request objects remain alive through cancellation so their abort signals
continue to propagate under garbage collection. Agent start and chat-send JSON requests allow up to 72 MiB to carry
the existing 25 MiB per-file and 50 MiB total attachment limits. Buffered control
and Agent request bodies share a 128 MiB gateway budget; other control requests
are limited to 1 MiB. HTTP application and service uploads stream with a 64 MiB
per-request limit. Disconnecting a browser request aborts its upstream transfer.
Each browser session allows 32 active HTTP requests and 32 browser WebSocket
connections, including upgrades still awaiting authorization or port resolution.
HTTP requests awaiting authorization count toward the same 32-request bound.
Pending WebSocket input is buffered up to 1 MiB per connection and forwarded in
order when the connection starts. Disconnected authorization attempts release
their slots, and unconsumed response bodies are cancelled on disconnect or a
failed authorization recheck.

HTML injection buffers at most 8 MiB per response and 32 MiB across a Desktop
host, cancels on disconnect and preserves empty HEAD/204/205/304 responses.
Supported compressed backend responses carry headers for their decoded body.
Range requests ask the backend for identity encoding; a backend that ignores
this and sends a compressed partial response receives a 502 response, since its
encoded byte offsets cannot be preserved after automatic decompression.
HTML partial responses are forwarded without script injection. Full HTML
injection removes validators/digests for the unmodified upstream body. HEAD
retains a correct length for unchanged representations and omits upstream
lengths for injected HTML or decoded compression.
Full HTML that still carries an unsupported content coding receives a 502
response before injection; encoded bytes are never rewritten as UTF-8 HTML.

Trusted optional ICE configuration is provided as JSON in
`LINGXI_LOCAL_APP_ICE_SERVERS`, for example an array containing a STUN entry and
TURN entries with short-lived `username` / `credential` values. TURN/TURNS URLs
and credential structure are validated. Provision/refresh these credentials in
the deployment environment; never put relay secrets in application manifests.
Without this setting the host uses the existing STUN defaults and HTTPS fallback.
Large-file RTC transfer and accelerated cookie-authenticated requests are not
implemented by this gateway.

## Configure an HTTPS tunnel

No cloud resources, domains or certificates are created by the application.
Provide a reachable relay, wildcard DNS and a valid wildcard certificate for a
Desktop-specific domain. The relay forwards encrypted HTTPS to a Desktop-side
TLS terminator. The HTTP gateway itself binds only to `127.0.0.1`.

The configuration generator uses frp's `https` proxy and `https2http` client
plugin. It preserves the public Host header and verifies the relay certificate
against the supplied CA. Authentication uses a token **file**, not a secret in
arguments, generated config text or repository files. See the upstream
[HTTPS plugin](https://gofrp.org/en/docs/examples/https2http/),
[client authentication and TLS fields](https://gofrp.org/en/docs/reference/client-configures/)
and [file value sources](https://gofrp.org/en/docs/reference/common/).

Prepare a private input JSON file outside the repository:

```json
{
  "serverAddr": "relay.example.com",
  "serverPort": 7000,
  "originTemplate": "https://{appId}-{sessionId}.desktop-one.apps.example.com",
  "gatewayPort": 17980,
  "tokenFile": "/private/deployment/relay-token",
  "trustedCaFile": "/private/deployment/relay-ca.pem",
  "certificateFile": "/private/deployment/apps-fullchain.pem",
  "privateKeyFile": "/private/deployment/apps-key.pem"
}
```

From `apps/electron` run:

```sh
npm run remote:tunnel:config -- /private/deployment/input.json /private/deployment/frpc.toml
frpc verify -c /private/deployment/frpc.toml
frpc -c /private/deployment/frpc.toml
```

The generator also writes `frpc.toml.desktop-env.json`. Apply its two values to
the Desktop process environment and restart that process:

```text
LINGXI_LOCAL_APP_REMOTE_ORIGIN=https://{appId}-{sessionId}.desktop-one.apps.example.com
LINGXI_LOCAL_APP_GATEWAY_PORT=17980
```

Configure frps to accept the authenticated client over verified TLS and to route
HTTPS on the origin's public port (normally 443). Its token must match the
private token file. This is deployment infrastructure, not a public anonymous
application endpoint. Preserve Host, do not enable request-body or secret-header
logging, and do not configure `subdomainHost` to conflict with the proxy's
wildcard `customDomains`. Certificate issuance/renewal, relay device enrollment,
resource quotas, monitoring and regional network acceptance belong to deployment.
The generator writes new files with mode 0600 and refuses to overwrite files.
It does not start a relay, download frp or read credential file contents.

Then enable Remote Control, pair the phone/browser, and enable **Allow access
from paired devices** on the desired Desktop application. On Mobile or Web,
open the connected computer's Apps catalog, refresh and select the app. Sharing
can be withdrawn from Desktop independently of background execution.
The remote catalog supports up to 256 shared applications. Enabling another
share at the limit gives an explicit error; deleting applications releases
their slots. Per-app and per-device withdrawal affects only its own grants.
An unsuccessful new opening clears its new ticket while preserving browser
sessions already in use during ordinary signaling loss.

For local browser tests only, an `http://{appId}-{sessionId}.localhost:<port>`
origin is accepted. Production remote clients require HTTPS. Browser access
still depends on the Desktop being awake and the configured tunnel being
reachable. Mobile hosting uses its existing Android/iOS runtime and background
mechanisms; no indefinite mobile background guarantee is introduced.

## Verification boundaries

Automated tests cover strict TS/Rust wire decoding, current-account/device/host
authorization across asynchronous launches, revocation generations, headless
runtime startup, release replacement, per-session origin separation, single-use
tickets, backend-cookie isolation, body bounds, streaming/WS revocation and RTC
framing, cancellation, deadlines and fallback. Real Chrome loopback gateway tests
exercise bootstrap, HttpOnly cookies, SDK invocation, separate reopen origins,
copied-cookie rejection and in-flight stream revocation without mocking fetch.

The browser test uses only temporary identities and a test resource adapter:

```sh
LINGXI_TEST_PLAYWRIGHT_MODULE=/absolute/path/to/playwright/index.mjs \
LINGXI_TEST_CHROME='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' \
node --import ../../packages/bridge-client/node_modules/tsx/dist/loader.mjs \
  --test test/local-app-remote-browser.test.mjs
```

Those results are distinct from an installed Desktop package, an actual internet
tunnel, real TURN connectivity, long-running device acceptance and native
permission UI. Android and iOS Rust FFI checks do not regenerate their bindings or
device libraries: rebuild the matching JNI/XCFramework artifacts with the normal
platform scripts before native compilation/installation. Existing archives
predate the new `listApps` / `openApp` exports and must not be treated as matching.
The optional iOS in-app browser also needs physical acceptance; Mobile Agent wake
cannot silently move pending IndexedDB input into a different browser. See the
[Mobile lifetime contract](MOBILE_LOCAL_APP_AGENT.md).
An already presented iOS remote browser is owned by the host view and remains
open across ordinary control reconnects. Pending launches are invalidated when
their catalog disappears; explicit dismissal clears the presented browser.
