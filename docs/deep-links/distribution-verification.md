# Deep-link distribution and acceptance

## Prepared artifacts

- `apps/web/public/.well-known/apple-app-site-association` uses iOS Team ID `AZ4AX7J833` and bundle IDs `com.lingxi.code` / `com.lingxi.code.full`, matching `apps/ios/native/project.yml`. Its `/open/*` allowlist covers the canonical native destinations; OAuth callback paths stay outside that prefix.
- `apps/web/public/.well-known/assetlinks.json` grants the verified local direct-release certificate to `com.lingxi.code.direct`. The public certificate has subject `O=lingxi, CN=lingxi`, validity 2026-10-19 through 2126-10-20 and SHA-256 `88:61:45:6D:EC:29:98:66:09:86:8D:27:94:2D:12:33:29:AE:5E:2C:A3:AA:43:20:54:B6:6A:34:02:98:46:54`. It was read from `docs/deep-links/android-direct-release-cert.pem` and checked with Node's X509 parser. No private key or password was read. No entry exists for `com.lingxi.code`: the real Play App Signing certificate is still unknown. The direct key is not evidence of Google's distribution signing identity.
- The approved AASA/direct association publication completed on 2026-10-19 at 03:19:23 UTC on existing Hosting site `lingxi-5e4b2` / domain `lingxios.net`. The default project was not changed. Only these two files and their exact-path JSON/five-minute-cache headers were added; existing website files and configuration were preserved.
- Windows portable packages include `register-protocol.ps1`. From the unpacked application directory, run `powershell -File .\register-protocol.ps1 Register`; remove the current user's association with `powershell -File .\register-protocol.ps1 Unregister` before moving or deleting the application. The helper writes only `HKCU:\Software\Classes\lingxi` and refuses to replace or delete another installation's command.
- Linux portable packages include executable `register-protocol.sh`. Run `./register-protocol.sh register`; remove its user registration with `./register-protocol.sh unregister` before moving or deleting the application. The helper writes only XDG user applications/configuration, quotes the executable path, restores the previous handler when available, and preserves unrelated MIME associations.
- macOS package verification checks `CFBundleURLTypes` in the assembled app's actual `Info.plist`. Signed builds must still use `npm run package:mac:flare` and the wrapper's static/runtime verification.

## Local evidence, 2026-10-19 UTC

- Shared corpus: 24 valid and 29 invalid inputs, checked by TypeScript and Rust tests. Covers every route family, session/message identity, prompts, multiline commands, import previews, duplicates, malformed encoding, nested credential fields, credential URLs with uppercase scheme, and pre-normalization path traversal.
- Electron: queue and actual main-handler tests cover cold launch, asynchronous workspace restoration, renderer/window recreation, second-instance duplicate delivery, FIFO acknowledgment, stale acknowledgments, queue limits, rejected subframes, startup error redaction, and stable message URL copying.
- Portable Linux script executes against temporary XDG directories and a simulated `xdg-mime`; registration, repeated registration, uninstall, previous-handler restoration, unrelated MIME preservation, escaped paths and ownership conflict refusal pass. Windows script content/packaging checks pass; no Windows registry execution occurred on this macOS host.
- macOS: real `plutil` tests verify scheme declaration and reject a mismatched scheme in the rewritten bundle plist. The existing local packaged artifact declares `lingxi`, bundle ID `com.lingxi.code.development`, Team ID `AZ4AX7J833`; this is inspection of an existing artifact, not a fresh build/runtime acceptance result.
- Direct association release preflight passes against the actual public direct signing certificate and current built files. The full preflight reports only the missing verified Play App Signing association.
- Web: 38 tests including association preflight regressions, TypeScript checks and production build pass. Browser smoke on local Vite preserves `session/AbC`, workspace and stable message UUID in the explicit `lingxi:` action; Copy produces the canonical production HTTPS URL and success status without navigation. Invalid payload renders no executable link or Copy button. No automatic app redirect occurs.

## Published association evidence

The user explicitly authorized publishing AASA and the verified direct Android association; Play publication was excluded. The Hosting REST API cloned live version `945c76230cdcf7d2` into `12801ce718ec1b78`, then populated only the two association files and appended two exact-path response-header rules. All existing security/cache headers and API/SPA rewrites were retained. The complete finalized file manifest proved that all 14,755 original paths retained their exact content hashes, with only two additions (14,757 total). One original rustdoc reference missing from the initial clone was restored using its original cached hash; no original website bytes were uploaded. Finalization and live release occurred only after complete manifest/configuration verification.

Live release: `1792379963147000`, version `12801ce718ec1b78`, time `2026-10-19T03:19:23.147Z`.

| Published URL | HTTP / MIME | SHA-256 of decoded content |
| --- | --- | --- |
| [AASA](https://lingxios.net/.well-known/apple-app-site-association) | 200 / application/json | `3c21133af4749d31c59b33d95a0e85654b344b9084042cab9325a248da2c937d` |
| [Direct Android association](https://lingxios.net/.well-known/assetlinks.json) | 200 / application/json | `c7f2d1929fb446b6fb524d1326ac7dedc3c8dc2642e7084745e9ad3d22e02490` |

Both canonical URLs were fetched with redirects disabled: neither returned a Location header, and each body exactly matched its approved source bytes. Both return `Cache-Control: public,max-age=300`. The Android response contains only `com.lingxi.code.direct` and the verified direct-release fingerprint; no Play package entry was published.

Immediately after this association-only release, the existing homepage retained SHA-256 `25c34c23dd8817fbae518229bb7e5b39167de60e3eb2b62a0db57d73b9f2a65c` and its original ETag. The separately authorized landing-page release below subsequently updated the entry HTML and its required JavaScript.

The method follows Firebase's official [clone](https://firebase.google.com/docs/reference/hosting/rest/v1beta1/sites.versions/clone), [populateFiles](https://firebase.google.com/docs/reference/hosting/rest/v1beta1/sites.versions/populateFiles), [version update](https://firebase.google.com/docs/reference/hosting/rest/v1beta1/sites.versions/patch), and [release](https://firebase.google.com/docs/reference/hosting/rest/v1beta1/sites.releases/create) APIs. Safe operational state and complete manifests are recorded locally in `/tmp/lingxi-association-hosting-state.json`; no access/refresh tokens were printed or stored in that evidence.

## Published landing-page evidence

The user separately authorized publishing the deeplink landing page, entry and required JS/CSS. Fresh Web verification passed 38 tests, both TypeScript checks and the production build. The old live and new built lazy admin chunks were compared in memory: their Firebase public configuration fields are identical (`lingxi-5e4b2`, `lingxi-5e4b2.firebaseapp.com`). The catalog API remains same-origin. No production environment file existed in this checkout, and no other project's configuration was used.

The REST API cloned the verified association version `12801ce718ec1b78` into `fbce9e4274b4d371`. Only three paths changed: `/index.html`, `/assets/index-CNaKWfPJ.js`, and `/assets/AdminModelsPage-xIY9H4tX.js`. The lazy admin chunk is required because its import points to the new main bundle. CSS, PNG and favicon compressed hashes were identical to live and needed no upload. All old hashed bundles remain available. The complete finalized manifest contains 14,759 paths: every one of the 14,757 original paths remains present, with only the entry HTML's existing hash changed and two new JS paths added. All rustdoc/static-doc paths, original response headers, API/SPA rewrites and other configuration are unchanged. Both association hashes above remain identical. The previous live version is retained for rollback.

Live release: `1792380732623000`, version `fbce9e4274b4d371`, time `2026-10-19T03:32:12.623Z`. One initial credentials-refresh request failed; retry reused the same verified draft and succeeded, without changing account/project or creating another clone.

| Updated path | HTTP / MIME | SHA-256 of decoded content |
| --- | --- | --- |
| `/index.html` | 200 / text/html | `e5aaf67a2edd9e7e69edf95582cbc6272693784c275d0fb6170310c25533c9a5` |
| `/assets/index-CNaKWfPJ.js` | 200 / text/javascript | `cd9786511e8c13ba31f6ea0ac73eaba292be3ac816764a3f1982fd219c7cfc8a` |
| `/assets/AdminModelsPage-xIY9H4tX.js` | 200 / text/javascript | `8836c84f175a0ab66a658f97c9469ccd211835533c2bf0ac3cd7fbce7a5cd5c8` |

Canonical HTTP requests disabled redirects and matched every approved artifact's bytes. Both AASA and direct assetlinks still return HTTP 200, `application/json`, no Location header, and their original approved content hashes. No Play association was added.

Real production Playwright browser checks passed:

- [New-session landing](https://lingxios.net/open/new) displays the valid destination and explicit `lingxi://new` Open action, with no automatic navigation. The Open control received an actual click; its default was prevented to verify the web action without claiming native OS handoff acceptance.
- A `session/CaseSensitive-ID` link preserves session/workspace case while normalizing only the uppercase message UUID to lowercase. Actual Copy writes the canonical production HTTPS URL, verified by reading the real browser clipboard, shows the success status, and does not navigate.
- The explicit Download link was clicked and reached `/download`.
- An import payload containing a dummy `apiKey` field displays the unavailable-link page with zero executable scheme actions and zero Copy actions. It does not redirect.
- The browser reported zero console errors or warnings for these checks.

Safe release state and complete manifests: `/tmp/lingxi-web-landing-hosting-state.json`. Browser evidence: `/tmp/lingxi-web-landing-production-browser.json`. These evidence files contain no access/refresh tokens. Production website/browser verification does not establish signed-app Universal Link/App Link, physical-device, or native protocol-handler behavior.

## Final evidence recheck

Read-only production recheck at `2026-10-19T03:42:08.514Z` confirmed the same live release `1792380732623000` / version `fbce9e4274b4d371`. Entry HTML, both new JS files and both association files still return HTTP 200 without Location headers and match every decoded-content hash recorded above. The associations still use `application/json`. A fresh real-browser visit to `/open/new` displays the valid destination, retains `lingxi://new` as its explicit Open action, and does not navigate automatically. No new deployment occurred.

Cached focused checks were rerun: shared TypeScript 4/4, shared Rust 4/4, Electron distribution/main/queue/packaging 41/41 and Web 38/38. Web's two TypeScript checks and Electron's Node TypeScript check passed. Rust executed the existing test binary built after the current source revision; no Cargo rebuild or primary product artifact rebuild was performed. The earlier production build evidence remains applicable because this follow-up changed only this evidence document.

## Message and child-transcript semantics

`session/<id>?message=<persistedUUID>` locates a row only in that session's main history. The message parameter is normalized to lowercase UUID notation; session, workspace and other resource identities preserve case. iOS main-history messages and tool rows can copy their persisted UUID link, which scrolls to and highlights the row. Child transcripts do not produce a parent-session message link that would point to a missing row. Their Copy agent link uses `runtime/agent/<id>` to open the full child transcript. No `agent:<id>` scope format was added.

iOS completed 110 pure checks, the matching three-slice native framework, app/test compilation, 196 focused native tests and five actual OS URL UI checks. The warm URL check retained the running process and its draft. The simulator build is ad-hoc signed without associated-domain entitlements, so signed-device Universal Link delivery remains a separate gate. Detailed logs and artifact provenance are recorded in [implementation-status.md](implementation-status.md).

## External acceptance gates

1. Obtain the actual Play App Signing SHA-256 certificate from the Play service or a verified Play-distributed APK. Add its `com.lingxi.code` association only after verification; keep the verified direct entry. Debug variants append `.debug` and are excluded.
2. Direct preflight: `node apps/web/scripts/verify-deep-link-release.mjs --variant direct --certificate com.lingxi.code.direct=docs/deep-links/android-direct-release-cert.pem`. Full Android preflight additionally requires `--certificate com.lingxi.code=/path/to/play-app-signing.der` and no variant restriction. The release validator rejects empty associations, missing required variants, unverified fingerprints, and stale built association files. The current full gate intentionally fails for the missing Play App Signing association; direct can pass independently.
3. AASA/direct publication and canonical HTTP verification are complete as recorded above. Future Play association changes require the actual Play App Signing identity and appropriate publication authorization; the direct certificate must not be substituted.
4. On signed physical iOS and Android installations, test HTTPS and scheme entry while cold, warm, foreground, background, and after a scene/activity recreation. Verify local resource resolution, UUID message position after restart, drafts without sending, missing-resource errors, import preview/cancel, and terminal editor confirmation. Compilation and parser tests do not prove these OS behaviors.
5. On native Windows and Linux hosts, build portable artifacts, verify registration/unregistration for the user, launch a cold link and deliver another to the running single instance. The current macOS environment does not establish Windows registry, Linux desktop integration or native package runtime acceptance.

There is no deferred web redirect, automatic post-install restoration, secret import, prompt submission, command execution, or extension installation in the distribution layer.
