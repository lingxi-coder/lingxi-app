#!/usr/bin/env python3
"""Generate iOS/Android localization resources from canonical translation JSON sources."""
import argparse
import json
import os
import re
import sys
import tempfile
from pathlib import Path

LOCALES = ["zh-Hans", "zh-Hant", "en", "ja", "ko"]
BASE_LOCALE = "zh-Hans"

ANDROID_VALUES_DIRS = {
    "zh-Hans": "values",
    "zh-Hant": "values-zh-rTW",
    "en": "values-en",
    "ja": "values-ja",
    "ko": "values-ko",
}

# Android format placeholder: %[<arg>$][flags][width][.precision]<conversion>
_ANDROID_PLACEHOLDER = re.compile(r"%(\d+\$)?[-+#0,]*\d*(\.\d+)?[a-zA-Z]")


def load_locales(directory: Path | str) -> dict[str, dict]:
    directory = Path(directory)
    locales: dict[str, dict] = {}
    for locale in LOCALES:
        path = directory / f"{locale}.json"
        if path.is_file():
            with open(path, encoding="utf-8-sig") as f:
                try:
                    locales[locale] = json.load(f)
                except json.JSONDecodeError as exc:
                    raise ValueError(f"error: {path} is not valid JSON: {exc}")
        else:
            locales[locale] = {}
    return locales


def validate_consistency(locales: dict[str, dict]) -> str | None:
    base = locales.get(BASE_LOCALE)
    if not isinstance(base, dict):
        return f"{BASE_LOCALE}: not an object"
    base_keys = set(base)
    problems = []
    for locale, strings in locales.items():
        if locale == BASE_LOCALE:
            continue
        if not isinstance(strings, dict):
            problems.append(f"{locale}: not an object")
            continue
        keys = set(strings)
        if not keys:
            continue
        base_info = base.get("__info_plist__")
        locale_info = strings.get("__info_plist__")
        if isinstance(base_info, dict) and isinstance(locale_info, dict) and base_info and locale_info:
            missing_inner = sorted(set(base_info) - set(locale_info))
            extra_inner = sorted(set(locale_info) - set(base_info))
            if missing_inner or extra_inner:
                detail = []
                if missing_inner:
                    detail.append("__info_plist__ missing keys: " + ", ".join(missing_inner))
                if extra_inner:
                    detail.append("__info_plist__ extra keys: " + ", ".join(extra_inner))
                problems.append(f"{locale}: " + "; ".join(detail))
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


def _localizations_for(locales: dict[str, dict], scope: str | None, key: str) -> dict:
    """Build the xcstrings localizations map for a key, skipping empty locales."""
    result = {}
    for locale in LOCALES:
        source = locales.get(locale)
        if not isinstance(source, dict):
            continue
        if scope:
            section = source.get(scope)
            value = section.get(key) if isinstance(section, dict) else None
        else:
            value = source.get(key)
        if value:
            result[locale] = {"stringUnit": {"state": "translated", "value": value}}
    return result


def write_ios(locales: dict[str, dict], out_dir: Path | str) -> None:
    """Emit Localizable.xcstrings into out_dir."""
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    base = locales.get(BASE_LOCALE, {})
    strings = {}
    for key in base:
        if key == "__info_plist__":
            info = base["__info_plist__"]
            if not isinstance(info, dict):
                continue
            for plist_key in info:
                localizations = _localizations_for(locales, "__info_plist__", plist_key)
                if localizations:
                    strings[plist_key] = {"localizations": localizations}
        else:
            localizations = _localizations_for(locales, None, key)
            if localizations:
                strings[key] = {"localizations": localizations}
    catalog = {
        "sourceLanguage": BASE_LOCALE,
        "version": "1.0",
        "strings": dict(sorted(strings.items())),
    }
    (out_dir / "Localizable.xcstrings").write_text(
        json.dumps(catalog, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


def _xml_escape_text(s: str) -> str:
    return (
        s.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace("'", "\\'")
        .replace('"', '\\"')
        .replace("%", "%%")
    )


def _android_escape(value: str) -> str:
    """Escape a string for Android strings.xml: double literal '%', escape quotes/apostrophes."""
    out = []
    last = 0
    for m in _ANDROID_PLACEHOLDER.finditer(value):
        out.append(_xml_escape_text(value[last : m.start()]))
        out.append(m.group(0))
        last = m.end()
    out.append(_xml_escape_text(value[last:]))
    return "".join(out)


def write_android(locales: dict[str, dict], out_dir: Path | str) -> None:
    """Emit strings.xml variants into out_dir (only for non-empty locales)."""
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    base = locales.get(BASE_LOCALE, {})
    keys = [k for k in base if k != "__info_plist__"]
    for locale in LOCALES:
        source = locales.get(locale, {})
        values = {}
        for key in keys:
            value = source.get(key) if isinstance(source, dict) else None
            if value:
                values[key] = value
        if not values:
            continue
        target = out_dir / ANDROID_VALUES_DIRS.get(locale, f"values-{locale}")
        target.mkdir(parents=True, exist_ok=True)
        lines = ['<?xml version="1.0" encoding="utf-8"?>', "<resources>"]
        for key, value in values.items():
            lines.append(f'    <string name="{key}">{_android_escape(value)}</string>')
        lines.append("</resources>")
        (target / "strings.xml").write_text("\n".join(lines) + "\n", encoding="utf-8")


def _stale_problems(locales: dict[str, dict], ios_out: Path, android_out: Path) -> list:
    """Regenerate into a temp dir and byte-diff against the committed outputs."""
    problems = []
    with tempfile.TemporaryDirectory() as tmp:
        tmp_dir = Path(tmp)
        write_ios(locales, tmp_dir / "ios")
        write_android(locales, tmp_dir / "android")
        generated = tmp_dir / "ios" / "Localizable.xcstrings"
        committed = ios_out / "Localizable.xcstrings"
        if not committed.exists():
            problems.append(f"error: missing generated file {committed}; run generate.py")
        elif generated.read_bytes() != committed.read_bytes():
            problems.append(f"error: {committed} is out of date; run generate.py")
        for xml in sorted((tmp_dir / "android").rglob("*.xml")):
            rel = xml.relative_to(tmp_dir / "android")
            committed = android_out / rel
            if not committed.exists():
                problems.append(f"error: missing generated file {committed}; run generate.py")
            elif xml.read_bytes() != committed.read_bytes():
                problems.append(f"error: {committed} is out of date; run generate.py")
    return problems


def main(argv=None):
    parser = argparse.ArgumentParser(description="Generate iOS/Android localization resources.")
    parser.add_argument("--ios-out", metavar="DIR", help="output directory for iOS Localizable.xcstrings")
    parser.add_argument("--android-out", metavar="DIR", help="output directory for Android strings.xml variants")
    parser.add_argument("--check", action="store_true", help="validate consistency only; write nothing")
    args = parser.parse_args(argv)

    repo_root = Path(__file__).resolve().parent.parent.parent
    try:
        locales = load_locales(Path(__file__).resolve().parent)
    except ValueError as exc:
        print(exc, file=sys.stderr)
        return 1

    error = validate_consistency(locales)
    if error:
        print(f"INCONSISTENT: {error}", file=sys.stderr)
        return 1

    ios_out = Path(args.ios_out) if args.ios_out else repo_root / "clients" / "ios" / "Resources"
    android_out = (
        Path(args.android_out) if args.android_out else repo_root / "clients" / "android" / "app" / "src" / "main" / "res"
    )

    if args.check:
        if not args.ios_out and not args.android_out:
            problems = _stale_problems(locales, ios_out, android_out)
            if problems:
                print("\n".join(problems), file=sys.stderr)
                return 1
    else:
        write_ios(locales, ios_out)
        write_android(locales, android_out)

    n_keys = len(locales.get(BASE_LOCALE, {}))
    print(f"OK: {n_keys} keys, {len(LOCALES)} locales")
    return 0


if __name__ == "__main__":
    sys.exit(main())
