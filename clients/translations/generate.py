#!/usr/bin/env python3
"""Generate iOS/Android localization resources from canonical translation JSON sources."""
import argparse
import json
import os
import sys

LOCALES = ["zh-Hans", "zh-Hant", "en", "ja", "ko"]
BASE_LOCALE = "zh-Hans"


def load_locales(directory):
    locales = {}
    for locale in LOCALES:
        path = os.path.join(directory, f"{locale}.json")
        if os.path.isfile(path):
            with open(path, encoding="utf-8") as f:
                locales[locale] = json.load(f)
        else:
            locales[locale] = {}
    return locales


def validate_consistency(locales):
    base_keys = set(locales.get(BASE_LOCALE, {}))
    problems = []
    for locale, strings in locales.items():
        if locale == BASE_LOCALE:
            continue
        keys = set(strings)
        if not keys:
            continue
        missing = sorted(base_keys - keys)
        extra = sorted(keys - base_keys)
        if missing or extra:
            detail = []
            if missing:
                detail.append("missing keys: " + ", ".join(missing))
            if extra:
                detail.append("extra keys: " + ", ".join(extra))
            problems.append(f"{locale}: " + "; ".join(detail))
    return "; ".join(problems) if problems else None


def write_ios(locales, out_dir):
    """Emit Localizable.xcstrings into out_dir (implemented in Task 2)."""
    pass


def write_android(locales, out_dir):
    """Emit strings.xml variants into out_dir (implemented in Task 2)."""
    pass


def main(argv=None):
    parser = argparse.ArgumentParser(description="Generate iOS/Android localization resources.")
    parser.add_argument("--ios-out", metavar="DIR", help="output directory for iOS Localizable.xcstrings")
    parser.add_argument("--android-out", metavar="DIR", help="output directory for Android strings.xml variants")
    parser.add_argument("--check", action="store_true", help="validate consistency only; write nothing")
    args = parser.parse_args(argv)

    locales = load_locales(os.path.dirname(os.path.abspath(__file__)))

    error = validate_consistency(locales)
    if error:
        print(f"INCONSISTENT: {error}", file=sys.stderr)
        return 1

    if not args.check:
        write_ios(locales, args.ios_out)
        write_android(locales, args.android_out)

    n_keys = len(locales.get(BASE_LOCALE, {}))
    print(f"OK: {n_keys} keys, {len(LOCALES)} locales")
    return 0


if __name__ == "__main__":
    sys.exit(main())
