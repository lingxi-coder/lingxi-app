# clients/translations

Canonical i18n source for the iOS and Android clients. One flat JSON file per
locale; a generator emits the platform-native formats both clients actually
build against. Nothing under `clients/ios/Resources/Localizable.xcstrings` or
`clients/android/app/src/main/res/values*/strings.xml` is hand-edited — those
are generated output, committed for a readable diff, and overwritten on every
`generate.py` run.

## Layout

```
clients/translations/
  zh-Hans.json     canonical source of truth — every other locale must have
                    the exact same key set as this file
  zh-Hant.json      繁體中文 (Taiwan Mandarin register)
  en.json           English
  ja.json           日本語
  ko.json           한국어
  generate.py       reads the *.json files, writes iOS + Android output
  test_generate.py  generator's own test suite (run with pytest or
                    `python3 test_generate.py -v`)
```

`zh-Hans` is the base locale: every key that exists there must exist, with
the same value shape, in every other non-empty locale file. An **empty**
locale file (`{}`) is exempt from that check — that's the state a locale sits
in before anyone has translated it, not an error.

## Key format

Flat, snake_case, prefixed by feature area: `chat_`, `composer_`,
`settings_`, `local_apps_`, `permission_`, `cron_`, `terminal_`, `voice_`,
`drawer_`, `onboarding_`, `project_`, `common_`, `app_`, `mcp_`, etc. Reuse an
existing key whenever a new UI string's source text is byte-identical to one
already in `zh-Hans.json` — this file has heavy, deliberate cross-feature and
cross-platform reuse (the same "取消"/"重试"/"设置" key backs both iOS and
Android call sites). Grep before inventing a new key.

```json
{
  "app_name": "灵犀",
  "common_cancel": "取消",
  "settings_language_title": "语言",
  "__info_plist__": {
    "CFBundleDisplayName": "灵犀",
    "NSCameraUsageDescription": "灵犀使用相机拍摄照片供模型分析。"
  }
}
```

- `__info_plist__` is iOS-only — the generator maps it into the String
  Catalog's InfoPlist string entries (camera/microphone/photo-library/speech
  permission descriptions + the display name). Android has no equivalent
  block; its `app_name` comes from the top-level key.
- `app_name`: zh-Hans/zh-Hant/ja/ko = "灵犀" (kept as the Chinese name
  across every locale by design — this is not a translation gap), en =
  "Lingxi".

### Placeholders — the two conventions are platform-specific, not interchangeable

**iOS**: the placeholder is embedded in the KEY itself, matching how Swift's
`String(localized:)` compiles a string-interpolation literal into a lookup
key — an interpolated Swift literal `"\(x) 天前"` where `x` is an `Int`
compiles to the key `"... %lld"`; a `String` interpolation compiles to
`"... %@"`. The **key** you write in every locale file must be that compiled
form (`"session_days_ago %lld"`), and the **value** carries the same token in
whatever position reads naturally for that language:

```json
"session_days_ago %lld": "%lld 天前"
```

**Android**: the placeholder lives only in the VALUE, using positional
`%1$s`/`%1$d`/`%2$s` etc. — the KEY stays plain snake_case. Positional
numbering exists specifically so a translation can reorder arguments to fit
different grammar without matching the source language's argument order:

```json
"cron_run_summary": "%1$s · Next %2$s"
```

Whichever convention applies, every locale's value for that key must contain
the exact same *set* of placeholder tokens the source does — same count, same
types — reordering is fine, dropping or duplicating one is not. `generate.py`
does not currently validate this automatically; check it by hand (or with a
short script) whenever you add or translate a key with a placeholder.

A source value may also contain a **real embedded newline** (not the two-char
`\n` escape — an actual line break inside the JSON string). The generator's
Android writer escapes it correctly into `strings.xml`'s `\n` form; a
translation carrying a multi-line source string should preserve the line
break in the same position.

## Adding a language

1. Add the canonical locale code to `LOCALES` in `generate.py` (currently
   `zh-Hans`, `zh-Hant`, `en`, `ja`, `ko`) and to the app-language mapping in
   `clients/ios/Sources/Theme/LocalizationManager.swift` and
   `clients/android/.../theme/AppLanguageStore.kt` (both map a persisted
   `xx-XX`-style app-language code, e.g. `fr-FR`, to the canonical resource
   locale, e.g. `fr`).
2. Create `clients/translations/<locale>.json` with every key from
   `zh-Hans.json`, translated. Copy `zh-Hans.json` as a starting skeleton
   (`cp zh-Hans.json <locale>.json`) so key order matches for a readable diff,
   then translate every value in place — do not add, remove, or rename keys.
3. Run the generator: `python3 clients/translations/generate.py` — must
   print `OK: <n> keys, 5 locales` (the count grows to 6+ once you've added a
   language slot). `--check` runs the same validation without writing files,
   useful in CI or before a commit.
4. Rebuild both clients so the new locale's generated resources
   (`Localizable.xcstrings` / `values-<locale>/strings.xml`) are picked up.

## Adding a key (extraction workflow)

1. Find the hardcoded literal in `clients/ios/Sources/**/*.swift` or
   `clients/android/app/src/main/java/**/*.kt`.
2. Grep `zh-Hans.json` for the exact source string — reuse the existing key
   if there's a match.
3. Otherwise, add `"feature_prefixed_key": "source text"` to
   **every non-empty** locale file (the base `zh-Hans.json` plus every locale
   that already has real translations — an empty `{}` locale is exempt and
   picks the key up whenever it's next translated).
4. Replace the source literal with the key, following each platform's
   convention (`Text("key")` / `String(localized: "key")` on iOS,
   `stringResource(R.string.key)` / `context.getString(R.string.key)` on
   Android — see the per-platform READMEs and the extraction commits on
   `feat/mobile-i18n` for the established patterns, including how to reach a
   localized string from code with no `Context`/no SwiftUI environment).
5. Run the generator, rebuild, verify no literal Han/target-script text
   remains at the call site.

## What NOT to extract

Not every string containing target-script text is UI copy:

- **Self-referential language names.** A language picker names each language
  in its own script regardless of the app's current locale — "日本語" reads
  "日本語" even in an English-language build. Both
  `LocalizationManager.swift`'s and `AppLanguageStore.kt`'s language-name
  arrays are deliberately hardcoded, not extracted.
- **Match-keys compared by `==`.** A few enum labels double as comparison
  targets elsewhere in the codebase (e.g. a sync-state label compared against
  a hardcoded string to pick a UI branch). Localizing one side without the
  other breaks the comparison in every non-source locale — either extract
  both sides through the same key, or leave both as the matching literal.
  Check call sites before extracting anything used in an equality check.
- **Internal substring/log matching.** A few error-classification functions
  do `message.contains("超时")` against raw engine text — never rendered,
  and translating a Chinese engine error's classifier does nothing for a
  user who never sees that string.
- **Mock/preview-only data.** Prototype fixture data reachable only from a
  SwiftUI preview, a `#if DEBUG` UI-test seam, or a "no engine connected"
  fallback path (grep for its actual construction sites before assuming it
  renders in a shipped build).
- **Dead code.** Verify a literal's owning function/property actually has a
  live call site before spending translation effort on it.

## Non-Composable / non-View string access

SwiftUI's `Text`/`String(localized:)` and Compose's `stringResource()` need a
view-layer environment. A plain `ViewModel()`, a callback body, or any class
JVM/XCTest unit tests construct directly with no `Context`/app environment
needs a different seam — this codebase's established pattern (see
`conversation/ConversationSource.kt`'s `ConversationStrings` /
`DefaultConversationStrings` / `conversationStrings(context)` on Android, or
the analogous `String(localized:)`-based helpers on iOS) is: a small resolver
type defaulting to the literal source-language fallback (so tests keep
passing with no environment), with a real, `Context`-backed implementation
wired at the type's **production construction site only**.

That production wiring is easy to get wrong silently: the default is a
valid, total implementation, so a call site that forgets to pass the real
resolver still compiles and still passes every test — it just never
localizes. Whenever you introduce or touch one of these seams, find its
production construction site and confirm by reading the actual call that the
real resolver is passed, not assumed.

## Testing localized output

A unit test asserting an exact piece of localized UI copy is implicitly
asserting a language too. On iOS, `String(localized:)` resolves against
whatever language the simulator/test host happens to be running — nothing in
the unit test target constructs `LocalizationManager.shared`, so its `Bundle`
swizzle never runs. `clients/ios/project.yml`'s test actions pin
`-AppleLanguages (zh-Hans)` as a launch argument for exactly this reason —
without it, results depend on the Mac's system language, not the code. On
Android, this class of flakiness mostly doesn't arise: a plain JVM unit test
can't call `Context.getString()` at all (no Robolectric runner in this
project's test suite), which is exactly why the resolver-with-fallback seam
above exists — tests exercise the fallback path deterministically, and the
real localized path only runs on-device.
