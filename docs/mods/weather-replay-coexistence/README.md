# Weather and Replay AbovePrompt composition

Both third-party Mods replaced the AbovePrompt tree, so opening Replay hid Weather.
The patch awaits downstream middleware and composes its tree with the Mod's own
content. The empty desktop engine marker is not inserted as a child. Empty draws
stay registered so later usage/edit invalidations can populate the site.

Desktop also waits for `bridge.connected` before mounting AbovePrompt. Otherwise
its first request can fail before the Bridge exists, leaving the band empty until
resize. The application change is in `apps/electron/src/renderer/App.tsx`.

## Third-party module patch

These modules are installed from `claude-code-playground-mods`, version 0.1.0;
they are not bundled Product sources. `above-prompt.patch` preserves the exact
coexistence fix applied to the user's installed modules, without modifying the
upstream marketplace or automatically rewriting plugin caches.

Apply from a directory containing `replay-theater/hooks/replay-theater.mjs` and
`token-weather/hooks/token-weather.mjs` (or a staging copy of those directories):

```sh
patch --dry-run -p1 < /path/to/above-prompt.patch
patch -p1 < /path/to/above-prompt.patch
node /path/to/verify.mjs ./replay-theater/hooks/replay-theater.mjs ./token-weather/hooks/token-weather.mjs
```

Check `module-hashes.json` before applying. The baseline is the locally repaired
0.1.0 modules: Replay already catches unavailable `$.ui.open` and falls back to
AbovePrompt; Weather already reads the current session usage. This patch is not
a replacement for those earlier fixes. Do not blindly apply it to another version.
Plugin reinstall/update can replace these local cache edits; this tracked patch
and its exact hashes preserve the fix for reapplication or an upstream change.

`verify.mjs` exercises both load orders through registered middleware: retained
downstream content, recording three file edits, hint, open, next, previous and
close. Close removes the expanded view and retains the reopen hint and Weather;
it does not unload the Mod or erase the last replay.

## Verification completed 2026-10-19

- Original modules reproduced Weather disappearing when Replay appeared.
- Patched modules passed both load orders and real Bun worker validation, including
  an initially empty draw becoming populated without nesting an engine marker.
- Production Bridge render and open/next/previous/close passed.
- Eleven focused Desktop tests and both TypeScript checks passed.
- Flare-signed macOS package passed static verification and packaged smoke.
- Actual signed Desktop with normal user data and DeepSeek Flash: Weather appeared
  on cold resume; two Write calls automatically showed Weather plus Replay without
  resizing. Open, next, previous and close retained Weather; no crash was observed.

Signed package ZIP SHA-256:
`835721442a9a06fb93ff0893253ae47775973e137114b92df63d3cfd11afcabf`.
Local detailed evidence: `.buildcache/mod-band-coexist-20261019/verification.json`
under the multi-repository workspace (not a tracked Product path).
