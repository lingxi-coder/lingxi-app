# Bot packages, Desktop installation and native handoff

P12/P14/P17 source implementation uses manifest schema 1 and the existing unified market v3 Bot resource. Local App source packages remain schema 2 and use their existing installer.

## Public export and publication

`main/bots/package.ts` builds a frozen package from the persona, explicitly checked text Skills, author supplied license text and public read-only resource requirement aliases. It never enumerates the Bot home or reads settings, model configuration, memory, sessions, credentials, private resource IDs or workspace bindings. Links, traversal, duplicate paths, executable/private files, unsupported manifest fields and recognized secret/local-path patterns are rejected. Pattern scans supplement the author's explicit review of every file and distribution rights.

The existing `marketClient.ts` HTTP transport and durable immutable publication journal now support both App and Bot collections. Fixed archive bytes, mutation bodies and idempotency keys precede remote mutations. Bot publication begins in the owner's library; audience policy/review remains the unified market's responsibility. The personal Bot Package & Market tab supports complete content review, ZIP export, own-library publication and recovery of the exact reviewed archive after restart. Recovery revalidates the archive and file digests, then requires another explicit review confirmation.

The existing ZIP/path/hash implementation accepts an explicit manifest identity so Bot import uses `lingxi-bot.json` without copying the archive implementation or calling the Local App backend installer. App archive, upload recovery and link regressions remain covered.

## Atomic installation and resource authorization

`BotMarketInstaller` authenticates the pinned v3 release, checks the release/ZIP/file hashes and strict manifest, downloads only through the canonical same-origin authenticated proxy, and validates all text before materialization. The existing personal Bot service calls BotStore to commit `bot.json` last, after preparing persona, text Skills, package manifest and fresh settings; it refreshes the ordinary Bot isolation settings and change notifications. The instance ID is randomly allocated by the existing BotStore.

`marketOrigin` records market/account/listing/release/digest and the local install identity. The local identity covers market/account/library/listing/release plus an explicit optional copy intent. Duplicate clicks share the existing install guard. Retries and lost ACKs locate the committed origin and return that same Bot; an explicit second-copy action allocates a distinct intent. No author model, resource binding, permission grant, remote setting, delegation or private data enters the new Bot.

Installed packages retain public required-resource declarations in `market-package.json`. The existing new SharedBot publication flow maps those requirements to the installer's own selected read-only resources and authorizes them before sharing. Download alone is neither enablement nor Team publication.

## Native and browser return

Canonical route:

```
lingxi://market/bot/{listingId}?versionId={releaseId}&intentId={receiptId}
https://lingxios.net/open/market/bot/{listingId}?versionId={releaseId}&intentId={receiptId}
```

Only public identities travel in the link. Both optional parameters are restricted to the Bot route. Existing App, Visualization and Mobile Local App hosting flows retain their existing routes and behavior.

Native iOS/Android reuse the system browser and platform share sheet. They preserve a UID-scoped pending command and pinned release before requesting the existing market's authenticated `bot-install-intents` receipt. Receipt creation uses the same saved idempotency key after interruption/ACK loss. A pending handoff can be reopened from the existing Apps library after relaunch. It works without selecting a local model or booting the local engine.

Desktop reads the UID-bound, expiring, currently authorized receipt, verifies the pinned release and ACKs it only after the personal Bot commits. The ACK uses the same instance ID, release digest and idempotency key after interruption. A receipt already completed on another Desktop does not create another instance. Native checks the authenticated receipt on return/manual refresh and reports installation only when the server confirms a committed Desktop instance. Enabling, Team sharing and execution remain distinct subsequent actions.

## Verification and remaining product gates

- Dedicated package/install/publication tests: 8 passed, including private export rejection, links, corruption/interrupted download, local duplicate clicks, explicit copy, publication ACK recovery, post-commit cross-device ACK loss and reviewed-archive recovery/tamper rejection.
- Combined dedicated and existing Bot store/content/service/IPC/render/preload, App ZIP/publication, market-link and canonical-link regression suite: **180 passed, 0 failed, 0 skipped**. The 8 dedicated cases above are included in this total.
- Rust shared canonical deep-link corpus: 5 passed.
- Pure Kotlin actual DeepLink + reusable MarketClient compilation and parser/corpus tests: 7 passed. This harness does not establish Android app/package/device acceptance.
- Electron node/web typecheck and production build passed after the installation/service integration and reviewed-publication recovery landed. The final recognized-secret scanner change also passed all 8 dedicated cases.
- Native lane confirmed the latest iOS **test build succeeded**, including authenticated ACK return, Apps resume entry and `BotMarketHandoffTests.swift`. Those iOS handoff tests have compiled; simulator/device execution has not yet occurred.
- Full Android app compilation currently waits on regeneration of stale generated runtime bindings (`MessageBlockDto.Page`), which still exists in the owning runtime schema. The valid source branch is preserved.

Signed Desktop packaging, actual authenticated market service operations, production identity/audience gates, physical device handoff and enable/share/reuse acceptance require their own evidence. Source/unit/simulator results do not establish those gates. No commit, push, deploy, credential change or production data cleanup is performed by this lane.
