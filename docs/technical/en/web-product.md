# Web · SDK site and UI prototype

The current browser application publishes SDK documentation and demonstrates account/console UI without a connected runtime backend.

<a id="web-current-boundary"></a>

## Current implementation boundary

apps/web is a React/Vite application with public pages, SDK documentation and a developer-console UI prototype. Documentation routes render local authored data and the generated source declaration index. Login displays an explicit prototype notice: submission sets UI state and navigates to the console; it sends no verification message and creates no real account.

Console account usage, balances and key records originate from mockData. API-key actions update local component state with prototype helpers, not a production authorization service. The site is not a hosted LingXi inference API, WebSocket Bridge host or remote agent session service. Working desktop/native execution is provided by their corresponding product hosts.

<a id="web-route-and-state"></a>

## Routes, local state and documentation data

The router normalizes path casing and separators, admits a fixed public/console route list and checks /docs/<id> against docPages. Unknown paths resolve to the public home page. App persists locale, region and theme in browser storage, updates document language and theme, and restores a section hash on route changes. Browser persistence is preference state, not durable runtime conversation storage.

Technical prose, bilingual structured tables, steps, code labels, SVG descriptions and source references live in src/data/docs. DocsPage consumes that schema for navigation, search, API browsing and section rendering; repository Markdown is generated from the same records. Keep factual tables in this data instead of encoding a second copy inside JSX.

| Route | Current data/source |
| --- | --- |
| /docs and /docs/<id> | Authored bilingual guide data and generated API/source index. |
| /login | Form prototype with local state and delayed navigation. |
| /console/* | Mock usage/billing/key records and local UI actions. |
| /download /pricing | Public product information and configured prototype records. |

<a id="web-build-and-hosting"></a>

## Build, preview and static hosting

Build runs Vite and writes dist. The repository-root Firebase Hosting configuration serves apps/web/dist with an index.html SPA fallback and forwards /v1/** to catalogApi. The model administration page is /admin/models. A successful static page response establishes frontend serving only; catalog/backend operation and runtime agent execution require separate acceptance.

Run documentation synchronization from the complete local workspace when refreshing source identity. Documentation-only CI can check saved content/output parity without sibling SDK checkouts. A stale API index is a documentation generation failure and should not be interpreted as a provider/network failure. Preserve current locale/theme behavior during schema additions.

### Website development from product root

```bash
npm --prefix apps/web ci
npm --prefix apps/web run docs:sync
npm --prefix apps/web run docs:check
npm --prefix apps/web run typecheck
npm --prefix apps/web test
npm --prefix apps/web run build
npm --prefix apps/web run dev -- --host 127.0.0.1
```

Verification method: Environment validation

<a id="web-verification"></a>

## Functional checks and open integrations

Check direct /docs/<id>#section entry, locale switching, search across translated prose, API filters, code copying, SVG accessibility text, light/dark themes and narrow-screen tables. Exported Markdown must retain the same content and links as the website. A route screenshot cannot establish auth, provider communication or a functioning marketplace/backend.

A future live console needs a separately specified account/session/credential backend and a browser-safe transport. Node BridgeClient imports ws and node:events and authenticates via a local lockfile header, so it cannot simply be imported into the current browser bundle. Those future decisions are integration work, not a capability already present in this site.

- [Local Node Bridge integration](bridge-client.md)
- [Desktop runtime host](desktop-host.md)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/lingxi-app/tree/main/apps/web/src/app/App.tsx)

- [lingxi-app/apps/web/src/app/App.tsx](https://github.com/lingxi-coder/lingxi-app/blob/f96f4d031ccdbee4ad8c90627b9d071d2cea435f/apps/web/src/app/App.tsx) — Committed source (f96f4d031ccdbee4ad8c90627b9d071d2cea435f)
- [lingxi-app/apps/web/src/app/router.ts](https://github.com/lingxi-coder/lingxi-app/blob/f96f4d031ccdbee4ad8c90627b9d071d2cea435f/apps/web/src/app/router.ts) — Committed source (f96f4d031ccdbee4ad8c90627b9d071d2cea435f)
- [lingxi-app/apps/web/src/app/usePersistentState.ts](https://github.com/lingxi-coder/lingxi-app/blob/f96f4d031ccdbee4ad8c90627b9d071d2cea435f/apps/web/src/app/usePersistentState.ts) — Committed source (f96f4d031ccdbee4ad8c90627b9d071d2cea435f)
- [lingxi-app/apps/web/src/pages/DocsPage.tsx](https://github.com/lingxi-coder/lingxi-app/blob/f96f4d031ccdbee4ad8c90627b9d071d2cea435f/apps/web/src/pages/DocsPage.tsx) — Committed source (f96f4d031ccdbee4ad8c90627b9d071d2cea435f)
- [lingxi-app/apps/web/src/pages/LoginPage.tsx](https://github.com/lingxi-coder/lingxi-app/blob/f96f4d031ccdbee4ad8c90627b9d071d2cea435f/apps/web/src/pages/LoginPage.tsx) — Committed source (f96f4d031ccdbee4ad8c90627b9d071d2cea435f)
- [lingxi-app/apps/web/src/pages/ConsolePages.tsx](https://github.com/lingxi-coder/lingxi-app/blob/f96f4d031ccdbee4ad8c90627b9d071d2cea435f/apps/web/src/pages/ConsolePages.tsx) — Committed source (f96f4d031ccdbee4ad8c90627b9d071d2cea435f)
- [lingxi-app/apps/web/src/data/mockData.ts](https://github.com/lingxi-coder/lingxi-app/blob/f96f4d031ccdbee4ad8c90627b9d071d2cea435f/apps/web/src/data/mockData.ts) — Committed source (f96f4d031ccdbee4ad8c90627b9d071d2cea435f)
- [lingxi-app/apps/web/src/utils/apiKeys.ts](https://github.com/lingxi-coder/lingxi-app/blob/f96f4d031ccdbee4ad8c90627b9d071d2cea435f/apps/web/src/utils/apiKeys.ts) — Committed source (f96f4d031ccdbee4ad8c90627b9d071d2cea435f)
- [lingxi-app/apps/web/package.json](https://github.com/lingxi-coder/lingxi-app/blob/f96f4d031ccdbee4ad8c90627b9d071d2cea435f/apps/web/package.json) — Committed source (f96f4d031ccdbee4ad8c90627b9d071d2cea435f)
- [lingxi-app/firebase.json](https://github.com/lingxi-coder/lingxi-app/blob/f96f4d031ccdbee4ad8c90627b9d071d2cea435f/firebase.json) — Committed source (f96f4d031ccdbee4ad8c90627b9d071d2cea435f)

- lingxi-app: HEAD f96f4d031ccdbee4ad8c90627b9d071d2cea435f; Referenced input fingerprint sha256:5a57b82938040552f6a17b7747394cd4a597eda384a65dfdc7f0a1d931df0613

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
