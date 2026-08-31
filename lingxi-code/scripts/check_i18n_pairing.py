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

## 三类静默丢失（每一类都在今天的树上真实存在）

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


def fail(msg):
    print("I18N-GATE FAIL: %s" % msg, file=sys.stderr)
    raise SystemExit(1)


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


def measure(locales):
    base = locales[BASE_LOCALE]
    keys = [k for k in base if k != "__info_plist__"]
    if not keys:
        fail("zh-Hans.json yielded ZERO keys — refusing to report a clean result from an empty enumeration")
    legal = {k for k in keys if ANDROID_NAME.match(k)}
    illegal = [k for k in keys if k not in legal]

    def stem(k):
        return k.split(" ")[0].split("%")[0].rstrip("_ ")

    unpaired = [k for k in illegal if stem(k) + ANDROID_PAIR_SUFFIX not in legal]
    m = {
        "totalKeys": len(keys),
        "androidIllegal": len(illegal),
        "androidIllegalUnpaired": len(unpaired),
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
    return problems


def main():
    ap = argparse.ArgumentParser(prog="check-i18n-pairing")
    here = Path(__file__).resolve()
    ap.add_argument("--dir", default=str(here.parents[2] / "clients" / "translations"))
    ap.add_argument("--baseline", default=str(here.parent / "i18n_pairing_baseline.json"))
    ap.add_argument("--update-baseline", action="store_true")
    ap.add_argument("--list", action="store_true")
    args = ap.parse_args()

    locales = load(args.dir)
    now = measure(locales)

    if args.list:
        print(json.dumps(now, indent=2, ensure_ascii=False))
        return 0
    if args.update_baseline:
        Path(args.baseline).write_text(json.dumps(now, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        print("baseline updated: %d keys, %d unpaired parameterized"
              % (now["totalKeys"], now["androidIllegalUnpaired"]))
        return 0

    bp = Path(args.baseline)
    if not bp.is_file():
        fail("baseline missing at %s — run with --update-baseline once, and commit it" % bp)
    problems = compare(json.loads(bp.read_text(encoding="utf-8")), now)
    if problems:
        fail("i18n regressions:\n  - " + "\n  - ".join(problems))
    print("OK: %d keys x %d locales; %d parameterized keys unpaired on Android (baseline, not growing)"
          % (now["totalKeys"], len(LOCALES), now["androidIllegalUnpaired"]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
