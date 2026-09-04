#!/usr/bin/env python3
"""i18n 门（驱动在 scripts/check-i18n-pairing.sh）。

与 scripts/check_deps.py / check_brand_leaks.py 同形状：只用 python3，规则是模块常量，
有 --list 模式，`sys.exit(main())`。

## 为什么 `generate.py --check` 不够

实测（在 lap/baseline 上跑出来的，不是读代码推的）：

    python3 generate.py --check                             -> OK: 1894 keys, 5 locales
    python3 generate.py --check --ios-out X --android-out Y  -> OK: 1894 keys, 5 locales

**后者一个字节都没有比对。** `stale_problems()` 在
`if not args.ios_out and not args.android_out:` 里面，而 `OK:` 那行 print 在
`try` 外面。两条路径的输出逐字节相同，退出码都是 0。§19.10 把
「打印 OK: … 5 locales」当作通过判据，于是这个判据可以由一条什么都不做的路径满足。

所以这个门做三件 `generate.py --check` 不做的事：

1. **禁掉那个参数组合**，并且自己断言 key 数，而不是相信那行字符串；
2. 把三类「静默丢失」的当前计数冻成基线，只允许下降；
3. 枚举为空时 fail-closed。

## 四类静默丢失（每一类都在今天的树上真实存在）

**a) Android 非法 key 名。** `generate.py` 的 `_ANDROID_NAME` 是
`[a-zA-Z_][a-zA-Z0-9_]*\\Z`，任何带空格或 `%` 的 key 被**直接丢弃**，不报错。
而 iOS 把占位符放在 **key** 里、Android 放在**值**里，所以每个带参文案在源里
长得就是 Android 非法的。今天 113 个这样的 key 里只有 7 个有显式的 `_fmt` 兄弟，
**106 个在 Android 侧完全没有对应 key**。

对着真实产物核过：1893 - 113 = 1780，而 `values/strings.xml` 里恰好 1780 个
`<string name=`。§17.5 会新增带参 key，所以这个数只能降不能升。

**b) 空值。** 空字符串在两端的产物里都会静默消失，而 key 数校验照样通过。

**c) 与 zh-Hans 逐字相同。** 未翻译。`generate.py` 只保证 key 齐，不保证翻译过；
§19.10 没有任何一条断言抓得到未翻译的 ja/ko。

## ⛔ 这个门不做的事

它**不**要求上面三个数变成 0。zh-Hant 有 227 条与 zh-Hans 相同，其中很多是
中文里本来就一样的词；把它们当缺陷会淹没真正的回归。判据是**基线不上涨**。

## d) 源码引用的 key 在产物目录里完全不存在

上面三类都假设 source 和 catalog 已经在互相看得见的范围内比对。它们看不到
**第四类**：iOS 源码里 `String(localized: "foo")` / `localized("foo")`（字面量,
不含 Swift 插值 —— 插值会被 Swift 自己编译成
`"foo %lld"`/`"foo %@"` 这种复合 key,那一半已经由 (a) 的 `_fmt` 配对逻辑覆盖）
引用的 `foo`,如果 `zh-Hans.json` 里根本没有这个 key,`generate.py` 不会报错
（它只保证「产出的 key 都被消费」的反方向从未被断言过）,`Localizable.xcstrings`
里也不会有这一条,运行时 `String(localized:)` 找不到匹配项时的行为是**原样显示
key 本身**——用户会在界面上看到 `settings_provider_login` 这种裸标识符。
Android 同理：`R.string.foo`（排除 `android.R.string.*` / `androidx.R.string.*`
框架资源）如果既不在 `zh-Hans.json` 的合法 key 集里、也不在
`values/strings_local_apps_v3.xml`（`local_app_runtime_profiles.rs` 之外，本仓库
唯一一份手工维护、由自己的重名冲突当 tripwire 的姊妹目录）里，资源合并会直接
报错——但只有在真的构建那个 flavor 时才会被发现。

2026-09-03 实测（在本 worktree 上跑出来的，不是读代码推的）：iOS 侧 48 个、
Android 侧 3 个（`chat_compacting_context` / `chat_compaction_failed` /
`chat_compaction_progress`——与 `ChatScreen.kt` 已知的三个不存在字符串键
是同一族，Android 主干本来就编不过，这道门只是第一次把它们放进一个会失败的
断言里）。判据同样是**基线不上涨**，不是**清零**——清零需要真的给这些 key
写译文，是内容决策，不是这道门的工作。
"""

import argparse
import json
import re
import sys
from pathlib import Path

LOCALES = ["zh-Hans", "zh-Hant", "en", "ja", "ko"]
BASE_LOCALE = "zh-Hans"

# 与 generate.py 的 `_ANDROID_NAME` 逐字一致。⚠️ 那边改了这边必须跟着改，
# 否则这个门会对着一个已经不存在的规则报告「没有回归」。
ANDROID_NAME = re.compile(r"[a-zA-Z_][a-zA-Z0-9_]*\Z")

# Android 侧配对用的后缀。实测出来的约定：`settings_provider_default %@`
# 对应 `settings_provider_default_fmt`，不是任何形式的名字改写。
ANDROID_PAIR_SUFFIX = "_fmt"

# 该被 §19.10 断言、而 `generate.py --check` 无法断言的参数组合。
FORBIDDEN_GENERATE_ARGS = ("--ios-out", "--android-out")

# 源码引用扫描的根目录，相对仓库根。
IOS_SOURCE_ROOT = "clients/ios/Sources"
ANDROID_SOURCE_ROOT = "clients/android/app/src/main/java"

# 手工维护、不经 generate.py 的姊妹资源文件——它们的 key 也算「已知」，
# 否则每一条被这份文件 RESTORE 回来的 key 都会被误报成 orphan。
ANDROID_COMPANION_STRINGS_FILES = (
    "clients/android/app/src/main/res/values/strings_local_apps_v3.xml",
)

# strings_local_apps_v3.xml 是手工维护的姊妹文件，不经 generate.py 生成，所以
# 不会像 values*/strings.xml 那样自动铺到每个 locale 目录——加一个 key 只加进
# values/ 很容易忘记其它 locale。ANDROID_LOCALE_DIR 把每个 LOCALES 条目映射到
# 它在 Android res 下应该有的 qualifier，这样才能查出「哪个 locale 的副本缺了
# 哪些 key」，而不只是「values/ 存在」。
# ⚠️ 与 generate.py 的 `ANDROID_VALUES_DIRS` 逐字一致。那边改了这边必须跟着改，
# 否则这个门会对着一个已经不存在的目录布局报告「没有回归」。迭代源是 LOCALES
# 而不是这个字典本身——字典缺条目要 fail-closed，不能静默少查一个 locale。
ANDROID_LOCALE_DIR = {
    "zh-Hans": "values",
    "zh-Hant": "values-zh-rTW",
    "en": "values-en",
    "ja": "values-ja",
    "ko": "values-ko",
}
ANDROID_COMPANION_STRINGS_BASENAME = "strings_local_apps_v3.xml"
ANDROID_RES_ROOT = "clients/android/app/src/main/res"

# 只匹配字面量 key（无 `\(...)` 插值）。插值调用会被 Swift 编译成复合 key
# （`"foo %lld"` 等），那一半已经由上面的 `_fmt` 配对逻辑覆盖，这里再匹配
# 只会重新引入本文件开头已经排除过的插值假阳性。
IOS_LOCALIZED_CALL = re.compile(
    r'String\(localized:\s*"([a-zA-Z_][a-zA-Z0-9_.]*)"\)|(?<![\w.])localized\("([a-zA-Z_][a-zA-Z0-9_.]*)"\)'
)
# 排除 `android.R.string.*` / `androidx.R.string.*` 框架资源——那些从不在
# 本仓库的 catalog 里，不是缺失。
ANDROID_RSTRING_REF = re.compile(r"\b(android\.|androidx\.)?R\.string\.([a-zA-Z_][a-zA-Z0-9_]*)")
ANDROID_STRING_NAME = re.compile(r'<string\s+name="([a-zA-Z_][a-zA-Z0-9_]*)"')

# Fail-closed 阈值：低于这个数说明 glob/regex 坏了（比如根目录改名），而不是
# 引用真的变少了。远低于 2026-09-03 的实测值（不同 key 数：iOS 969、Android 1216），只挡
# 「扫到 0」这类枚举事故。
MIN_IOS_SOURCE_REFS = 500
MIN_ANDROID_SOURCE_REFS = 800


def fail(msg):
    print("I18N-GATE FAIL: %s" % msg, file=sys.stderr)
    raise SystemExit(1)


def _ios_source_refs(repo_root):
    """key -> 第一处引用它的文件路径（字符串字面量调用，不含插值）。"""
    refs = {}
    root = Path(repo_root) / IOS_SOURCE_ROOT
    for path in sorted(root.rglob("*.swift")):
        text = path.read_text(encoding="utf-8", errors="replace")
        for m in IOS_LOCALIZED_CALL.finditer(text):
            key = m.group(1) or m.group(2)
            refs.setdefault(key, str(path))
    return refs


def _android_source_refs(repo_root):
    """key -> 第一处引用它的文件路径（排除 android(x).R.string.* 框架资源）。"""
    refs = {}
    root = Path(repo_root) / ANDROID_SOURCE_ROOT
    for path in sorted(root.rglob("*.kt")):
        text = path.read_text(encoding="utf-8", errors="replace")
        for m in ANDROID_RSTRING_REF.finditer(text):
            if m.group(1):
                continue
            refs.setdefault(m.group(2), str(path))
    return refs


def _android_companion_names(repo_root):
    """`values/strings_local_apps_v3.xml` 等手工维护姊妹文件里定义的 key 名。"""
    names = set()
    for rel in ANDROID_COMPANION_STRINGS_FILES:
        path = Path(repo_root) / rel
        if path.is_file():
            names.update(ANDROID_STRING_NAME.findall(path.read_text(encoding="utf-8", errors="replace")))
    return names


def _android_companion_locale_gaps(repo_root):
    """locale -> sorted missing key list: keys `values/strings_local_apps_v3.xml`
    defines that this locale's own ANDROID_LOCALE_DIR copy does not have (a
    missing file counts every base key as missing). Base locale excluded —
    it can't be missing from itself."""
    root = Path(repo_root) / ANDROID_RES_ROOT
    missing_dir_map = [loc for loc in LOCALES if loc not in ANDROID_LOCALE_DIR]
    if missing_dir_map:
        fail("ANDROID_LOCALE_DIR has no entry for locale(s) %s — mirror generate.py's "
             "ANDROID_VALUES_DIRS, otherwise those locales are never checked for a "
             "%s copy and this gate reports OK while they silently fall back to %s"
             % (", ".join(repr(loc) for loc in missing_dir_map),
                ANDROID_COMPANION_STRINGS_BASENAME, BASE_LOCALE))
    base_path = root / ANDROID_LOCALE_DIR[BASE_LOCALE] / ANDROID_COMPANION_STRINGS_BASENAME
    base_keys = (
        set(ANDROID_STRING_NAME.findall(base_path.read_text(encoding="utf-8", errors="replace")))
        if base_path.is_file() else set()
    )
    gaps = {}
    for loc in LOCALES:
        if loc == BASE_LOCALE:
            continue
        dirname = ANDROID_LOCALE_DIR[loc]
        p = root / dirname / ANDROID_COMPANION_STRINGS_BASENAME
        have = (
            set(ANDROID_STRING_NAME.findall(p.read_text(encoding="utf-8", errors="replace")))
            if p.is_file() else set()
        )
        missing = sorted(base_keys - have)
        if missing:
            gaps[loc] = missing
    return gaps


def load(directory):
    directory = Path(directory)
    out = {}
    for loc in LOCALES:
        p = directory / ("%s.json" % loc)
        if not p.is_file():
            # generate.py 把缺失的 locale 当 `{}` 加载，而 validate_consistency
            # 会跳过空 key 集——于是删掉 ja.json 也能通过，还照样报 "5 locales"。
            fail("%s is missing. generate.py loads a missing locale as {} and still reports "
                 "'%d locales', so absence is invisible there." % (p, len(LOCALES)))
        out[loc] = json.loads(p.read_text(encoding="utf-8"))
    return out


def measure(locales, repo_root):
    base = locales[BASE_LOCALE]
    keys = [k for k in base if k != "__info_plist__"]
    if not keys:
        fail("zh-Hans.json yielded ZERO keys — refusing to report a clean result from an empty enumeration")
    key_set = set(keys)
    legal = {k for k in keys if ANDROID_NAME.match(k)}
    illegal = [k for k in keys if k not in legal]

    def stem(k):
        return k.split(" ")[0].split("%")[0].rstrip("_ ")

    unpaired = [k for k in illegal if stem(k) + ANDROID_PAIR_SUFFIX not in legal]

    ios_refs = _ios_source_refs(repo_root)
    if len(ios_refs) < MIN_IOS_SOURCE_REFS:
        fail("found only %d distinct key(s) referenced by iOS String(localized:)/localized() under %s "
             "— refusing to trust an enumeration this much smaller than the known count, the scan "
             "is probably broken"
             % (len(ios_refs), Path(repo_root) / IOS_SOURCE_ROOT))
    android_refs = _android_source_refs(repo_root)
    if len(android_refs) < MIN_ANDROID_SOURCE_REFS:
        fail("found only %d distinct key(s) referenced by Android R.string.* under %s — refusing to "
             "trust an enumeration this much smaller than the known count, the scan is probably broken"
             % (len(android_refs), Path(repo_root) / ANDROID_SOURCE_ROOT))
    android_known = legal | _android_companion_names(repo_root)
    ios_orphans = sorted(k for k in ios_refs if k not in key_set)
    android_orphans = sorted(k for k in android_refs if k not in android_known)
    companion_gaps = _android_companion_locale_gaps(repo_root)

    m = {
        "totalKeys": len(keys),
        "androidIllegal": len(illegal),
        "androidIllegalUnpaired": len(unpaired),
        "iosSourceOrphans": len(ios_orphans),
        "androidSourceOrphans": len(android_orphans),
        "androidCompanionLocaleGapsTotal": sum(len(v) for v in companion_gaps.values()),
        "empty": {},
        "identicalToBase": {},
    }
    for loc in LOCALES:
        d = locales[loc]
        m["empty"][loc] = sum(1 for k in keys if d.get(k, "") == "")
        m["identicalToBase"][loc] = (
            0 if loc == BASE_LOCALE
            else sum(1 for k in keys if d.get(k) is not None and d.get(k) == base.get(k))
        )
    # 存**全量**而不是采样。基线只有 106 条,存全量的代价可以忽略,
    # 而采样会让新增的那个 key 落在样本外——门就只能说「多了一个」,
    # 说不出多了哪个。「点名那个具体的东西」是本仓库所有门的判据。
    m["unpairedKeys"] = sorted(unpaired)
    m["iosSourceOrphanKeys"] = ios_orphans
    m["androidSourceOrphanKeys"] = android_orphans
    m["androidCompanionLocaleGaps"] = companion_gaps
    return m


def compare(base, now):
    problems = []
    if now["totalKeys"] != base["totalKeys"]:
        # key 数变化本身不是错——加文案就会变。它只是必须被看见。
        print("I18N-GATE NOTE: key count moved %d -> %d" % (base["totalKeys"], now["totalKeys"]))
    if now["androidIllegalUnpaired"] > base["androidIllegalUnpaired"]:
        problems.append(
            "%d parameterized key(s) now have NO Android counterpart, baseline was %d. "
            "These are dropped from strings.xml silently and generate.py --check still prints OK. "
            "Each new one needs a '%s' sibling. New unpaired keys include: %s"
            % (now["androidIllegalUnpaired"], base["androidIllegalUnpaired"], ANDROID_PAIR_SUFFIX,
               ", ".join(repr(k) for k in sorted(set(now["unpairedKeys"]) - set(base.get("unpairedKeys", [])))))
        )
    for loc in LOCALES:
        if now["empty"][loc] > base["empty"][loc]:
            problems.append(
                "%s has %d empty value(s), baseline %d — an empty value vanishes from BOTH generated "
                "outputs while the key-count check still passes"
                % (loc, now["empty"][loc], base["empty"][loc])
            )
        if now["identicalToBase"][loc] > base["identicalToBase"][loc]:
            problems.append(
                "%s has %d value(s) byte-identical to %s, baseline %d — untranslated; nothing in "
                "generate.py --check would report this"
                % (loc, now["identicalToBase"][loc], BASE_LOCALE, base["identicalToBase"][loc])
            )
    if now["iosSourceOrphans"] > base.get("iosSourceOrphans", 0):
        problems.append(
            "%d distinct key(s) referenced by iOS String(localized:)/localized() have NO entry "
            "in %s.json, baseline was %d — String(localized:) falls back to displaying the raw key "
            "text when the catalog has no match. New orphans include: %s"
            % (now["iosSourceOrphans"], BASE_LOCALE, base.get("iosSourceOrphans", 0),
               ", ".join(repr(k) for k in sorted(
                   set(now["iosSourceOrphanKeys"]) - set(base.get("iosSourceOrphanKeys", []))
               )))
        )
    if now["androidSourceOrphans"] > base.get("androidSourceOrphans", 0):
        problems.append(
            "%d distinct key(s) referenced by Android R.string.* have NO entry in %s.json and no "
            "sibling in %s, baseline was %d — the resource merger fails at build time for the flavor "
            "that hits it. New orphans include: %s"
            % (now["androidSourceOrphans"], BASE_LOCALE, ", ".join(ANDROID_COMPANION_STRINGS_FILES),
               base.get("androidSourceOrphans", 0),
               ", ".join(repr(k) for k in sorted(
                   set(now["androidSourceOrphanKeys"]) - set(base.get("androidSourceOrphanKeys", []))
               )))
        )
    if now["androidCompanionLocaleGapsTotal"] > base.get("androidCompanionLocaleGapsTotal", 0):
        detail = "; ".join(
            "%s missing %d key(s) (%s)" % (
                loc, len(keys),
                ", ".join(repr(k) for k in keys[:5]) + (", …" if len(keys) > 5 else "")
            )
            for loc, keys in sorted(now.get("androidCompanionLocaleGaps", {}).items())
        )
        problems.append(
            "%d strings_local_apps_v3.xml key(s) missing from a non-base Android locale copy "
            "under %s, baseline was %d — that locale's resource resolution silently falls back to "
            "values/ (%s) for these keys, e.g. the AskUserQuestion card copy shows %s instead of the "
            "device language. %s"
            % (now["androidCompanionLocaleGapsTotal"], ANDROID_RES_ROOT,
               base.get("androidCompanionLocaleGapsTotal", 0), BASE_LOCALE, BASE_LOCALE, detail)
        )
    return problems


def main():
    ap = argparse.ArgumentParser(prog="check-i18n-pairing")
    here = Path(__file__).resolve()
    ap.add_argument("--dir", default=str(here.parents[2] / "clients" / "translations"))
    ap.add_argument("--repo-root", default=str(here.parents[2]))
    ap.add_argument("--baseline", default=str(here.parent / "i18n_pairing_baseline.json"))
    ap.add_argument("--update-baseline", action="store_true")
    ap.add_argument("--list", action="store_true")
    args = ap.parse_args()

    locales = load(args.dir)
    now = measure(locales, args.repo_root)

    if args.list:
        print(json.dumps(now, indent=2, ensure_ascii=False))
        return 0
    if args.update_baseline:
        Path(args.baseline).write_text(json.dumps(now, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        print("baseline updated: %d keys, %d unpaired parameterized, %d iOS + %d Android source orphans"
              % (now["totalKeys"], now["androidIllegalUnpaired"], now["iosSourceOrphans"],
                 now["androidSourceOrphans"]))
        return 0

    bp = Path(args.baseline)
    if not bp.is_file():
        fail("baseline missing at %s — run with --update-baseline once, and commit it" % bp)
    problems = compare(json.loads(bp.read_text(encoding="utf-8")), now)
    if problems:
        fail("i18n regressions:\n  - " + "\n  - ".join(problems))
    print("OK: %d keys x %d locales; %d parameterized keys unpaired on Android, %d iOS + %d Android "
          "source-reference orphans (all baseline, none growing)"
          % (now["totalKeys"], len(LOCALES), now["androidIllegalUnpaired"], now["iosSourceOrphans"],
             now["androidSourceOrphans"]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
