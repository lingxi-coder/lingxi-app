import json, subprocess, tempfile, os, sys
from pathlib import Path
sys.path.insert(0, os.path.dirname(__file__))
from generate import TEAMS_TEXT_LOCALES, load_locales, render_teams_text, scan_teams_keys, stale_problems, validate_consistency, write_android, write_ios, write_teams_text

def test_consistency_ok():
    locales = load_locales(os.path.dirname(__file__))
    assert validate_consistency(locales) is None  # empty locales trivially consistent

def test_consistency_catches_missing_key():
    locales = {"zh-Hans": {"a": "甲", "b": "乙"}, "en": {"a": "甲"}}
    err = validate_consistency(locales)
    assert err and "en" in err and "b" in err

def test_consistency_catches_extra_key():
    locales = {"zh-Hans": {"a": "甲"}, "en": {"a": "甲", "c": "丙"}}
    err = validate_consistency(locales)
    assert err and "en" in err and "c" in err

def test_consistency_non_dict():
    locales = {"zh-Hans": {"a": "甲"}, "en": "not an object"}
    err = validate_consistency(locales)
    assert err and "en" in err and "not an object" in err

def test_consistency_info_plist_inner_parity():
    locales = {
        "zh-Hans": {"a": "甲", "__info_plist__": {"CFBundleDisplayName": "灵犀", "NSCameraUsageDescription": "相机"}},
        "en": {"a": "A", "__info_plist__": {"CFBundleDisplayName": "Lingxi"}},
    }
    err = validate_consistency(locales)
    assert err and "en" in err and "NSCameraUsageDescription" in err

def test_consistency_rejects_missing_duplicate_or_wrong_format_arguments():
    for translated in ("Used %1$d", "Used %1$d %1$d %2$s", "Used %1$s %2$s"):
        error = validate_consistency({"zh-Hans": {"summary": "%1$d · %2$s"}, "en": {"summary": translated}})
        assert error and "summary: formatting arguments differ" in error

def test_consistency_allows_reordered_arguments_and_escaped_percent():
    assert validate_consistency({
        "zh-Hans": {"summary": "%1$d%% · %2$s"},
        "en": {"summary": "%2$s · %1$d%%"},
    }) is None

def test_consistency_checks_each_plural_variation():
    error = validate_consistency({
        "zh-Hans": {"count %lld": "%lld 项"},
        "en": {"count %lld": {"plural": {"one": "%lld item", "other": "items"}}},
    })
    assert error and "count %lld: formatting arguments differ" in error

def test_generate_check_exit_0(tmp_path):
    out = tmp_path / "out"
    result = subprocess.run([sys.executable, "generate.py", "--ios-out", str(out), "--android-out", str(out), "--check"], cwd=os.path.dirname(__file__), capture_output=True, text=True)
    assert result.returncode == 1
    assert "--check cannot be combined" in result.stderr

def test_generate_check_rejects_custom_output_dirs(tmp_path):
    out = tmp_path / "out"
    result = subprocess.run(
        [sys.executable, "generate.py", "--check", "--ios-out", str(out)],
        cwd=os.path.dirname(__file__),
        capture_output=True,
        text=True,
    )
    assert result.returncode == 1
    assert "--check cannot be combined" in result.stderr

def test_ios_catalog_shape(tmp_path):
    rc = subprocess.run([sys.executable, "generate.py", "--ios-out", str(tmp_path), "--android-out", str(tmp_path / "android")], cwd=os.path.dirname(__file__)).returncode
    assert rc == 0
    cat = json.load(open(tmp_path / "Localizable.xcstrings"))
    assert cat["sourceLanguage"] == "zh-Hans"
    assert cat["version"] == "1.0"
    assert "app_name" in cat["strings"]
    # System permission dialogs look in InfoPlist.strings, not Localizable.
    info = json.load(open(tmp_path / "InfoPlist.xcstrings"))
    assert "NSCameraUsageDescription" in info["strings"]
    en = cat["strings"]["app_name"]["localizations"]["en"]["stringUnit"]["value"]
    assert en == "LingXi"

def test_android_resources(tmp_path):
    rc = subprocess.run([sys.executable, "generate.py", "--ios-out", str(tmp_path / "ios"), "--android-out", str(tmp_path)], cwd=os.path.dirname(__file__)).returncode
    assert rc == 0
    xml = open(tmp_path / "values-en" / "strings.xml").read()
    assert '<string name="app_name">LingXi</string>' in xml

def test_android_escaping(tmp_path):
    locales = {
        "zh-Hans": {
            "k": "It's 5% sure",
            "n": "共 %1$d 个",
            "q": 'a "b"',
            "a": "a <b> & c",
            "p": "100%x",
            "nl": "line one\nline two",
            "percent": "Used %1$d%% (remaining %2$d%%)",
            "percent_letter": "100%%x",
        },
        "en": {"k": "x", "n": "y", "q": "z", "a": "w", "p": "v", "nl": "u"},
    }
    write_android(locales, tmp_path)
    xml = open(tmp_path / "values" / "strings.xml").read()
    assert '<string name="k">It\\\'s 5%% sure</string>' in xml
    assert '<string name="n">共 %1$d 个</string>' in xml
    assert '<string name="q">a \\"b\\"</string>' in xml
    assert '<string name="a">a &lt;b&gt; &amp; c</string>' in xml
    assert '<string name="p">100%x</string>' in xml
    # A literal newline in the source value must become the two-character
    # `\n` escape, not a raw line break in the XML text node — Android's
    # resource format requires the escape to render it as a line break.
    assert '<string name="nl">line one\\nline two</string>' in xml
    assert "line one\nline two" not in xml
    assert '<string name="percent">Used %1$d%% (remaining %2$d%%)</string>' in xml
    assert '<string name="percent_letter">100%%x</string>' in xml

def _gate_sandbox(tmp_path):
    """Write canonical locales into a tmp dir and return (locales, ios_out, android_out)."""
    src = tmp_path / "locales"
    src.mkdir()
    (src / "zh-Hans.json").write_text(
        json.dumps({"app_name": "灵犀", "__info_plist__": {"CFBundleDisplayName": "灵犀"}}, ensure_ascii=False), encoding="utf-8"
    )
    (src / "en.json").write_text(
        json.dumps({"app_name": "Lingxi", "__info_plist__": {"CFBundleDisplayName": "灵犀"}}, ensure_ascii=False), encoding="utf-8"
    )
    locales = load_locales(src)
    return locales, tmp_path / "ios-out", tmp_path / "android-out"

def test_stale_gate_clean(tmp_path):
    locales, ios_out, android_out = _gate_sandbox(tmp_path)
    write_ios(locales, ios_out)
    write_android(locales, android_out)
    assert stale_problems(locales, ios_out, android_out) == []

def test_stale_gate_out_of_date(tmp_path):
    locales, ios_out, android_out = _gate_sandbox(tmp_path)
    write_ios(locales, ios_out)
    write_android(locales, android_out)
    cat = ios_out / "Localizable.xcstrings"
    cat.write_text(cat.read_text().replace("Lingxi", "LingxiX"), encoding="utf-8")
    problems = stale_problems(locales, ios_out, android_out)
    assert any("Localizable.xcstrings" in p and "out of date" in p for p in problems)

def test_info_plist_catalog_is_separate_and_freshness_checked(tmp_path):
    locales, ios_out, android_out = _gate_sandbox(tmp_path)
    write_ios(locales, ios_out)
    write_android(locales, android_out)
    ui = json.loads((ios_out / "Localizable.xcstrings").read_text())
    info_path = ios_out / "InfoPlist.xcstrings"
    info = json.loads(info_path.read_text())
    assert "CFBundleDisplayName" not in ui["strings"]
    assert info["strings"]["CFBundleDisplayName"]["localizations"]["en"]["stringUnit"]["value"] == "灵犀"
    info_path.unlink()
    assert any("InfoPlist.xcstrings" in p and "missing" in p for p in stale_problems(locales, ios_out, android_out))
    write_ios(locales, ios_out)
    info_path.write_text("{}", encoding="utf-8")
    assert any("InfoPlist.xcstrings" in p and "out of date" in p for p in stale_problems(locales, ios_out, android_out))

def test_stale_gate_orphan(tmp_path):
    locales, ios_out, android_out = _gate_sandbox(tmp_path)
    write_ios(locales, ios_out)
    write_android(locales, android_out)
    orphan = android_out / "values-ja" / "strings.xml"
    orphan.parent.mkdir(parents=True, exist_ok=True)
    orphan.write_text("<resources></resources>", encoding="utf-8")
    problems = stale_problems(locales, ios_out, android_out)
    assert any("orphaned" in p and "values-ja" in p for p in problems)

def test_generate_check_clean_repo_gate():
    result = subprocess.run([sys.executable, "generate.py", "--check"], cwd=os.path.dirname(__file__), capture_output=True, text=True)
    assert result.returncode == 0
    assert "OK:" in result.stdout

def test_generate_check_rejects_orphan_catalog_keys(tmp_path):
    sandbox_locales, ios_out, android_out = _gate_sandbox(tmp_path)
    write_ios(sandbox_locales, ios_out)
    write_android(sandbox_locales, android_out)
    catalog_path = ios_out / "Localizable.xcstrings"
    catalog = json.loads(catalog_path.read_text(encoding="utf-8"))
    catalog["strings"]["orphan_key"] = {}
    catalog_path.write_text(json.dumps(catalog, ensure_ascii=False), encoding="utf-8")
    problems = stale_problems(sandbox_locales, ios_out, android_out)
    assert any("Localizable.xcstrings" in problem and "out of date" in problem for problem in problems)

def _teams_sources(root, files):
    """Write `files` ({relative path: text}) under `root` and return it, to be scanned for the Teams text bundle."""
    for rel, text in files.items():
        target = root / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8")
    return root

def _teams_locales(**entries):
    """Every Teams locale holds `entries` ({key: {locale: text}}); the Desktop and Web views read all five."""
    locales = {locale: {"__info_plist__": {"CFBundleDisplayName": "灵犀"}} for locale in TEAMS_TEXT_LOCALES}
    for key, by_locale in entries.items():
        for locale, text in by_locale.items():
            locales[locale][key] = text
    return locales

def test_teams_scan_reads_key_calls_and_family_literals_of_the_teams_sources(tmp_path):
    root = _teams_sources(tmp_path, {
        "packages/teams/View.tsx": "const a = t('teams_title'); const b = tk(\"friends_title\");\nconst c = ['tw_back_team'];\n",
        "packages/teams/View.test.ts": "t('teams_only_in_a_test')\n",
        "packages/teams/i18n.generated.ts": "t('teams_in_the_bundle_itself')\n",
        "apps/web/src/Host.tsx": "import { useTeamsText } from '../../../packages/teams/i18n';\nconst x = t('services_menu');\n",
        "apps/web/src/Other.tsx": "const y = t('teams_not_a_teams_host');\n",
        "apps/web/src/Plain.tsx": "const z = t('Save', 'Speichern'); const w = t('not-a-key');\n",
    })
    refs = scan_teams_keys(root)
    assert sorted(refs) == ["friends_title", "services_menu", "teams_title", "tw_back_team"]
    assert refs["teams_title"] == ["packages/teams/View.tsx:1"]
    assert refs["tw_back_team"] == ["packages/teams/View.tsx:2"]
    assert refs["services_menu"] == ["apps/web/src/Host.tsx:2"]

def test_teams_bundle_holds_each_referenced_key_in_five_locales_with_escaped_text():
    text = {"en": "It's\nhere", "zh-Hans": "在这里", "zh-Hant": "在這裡", "ja": "ここ", "ko": "여기"}
    locales = _teams_locales(b_key={locale: "B" for locale in TEAMS_TEXT_LOCALES}, a_key=text)
    bundle = render_teams_text(locales, {"b_key": ["x.tsx:1"], "a_key": ["x.tsx:2"]})
    assert "export type TeamsLocale = 'en' | 'zh-Hans' | 'zh-Hant' | 'ja' | 'ko';" in bundle
    assert bundle.index("'a_key'") < bundle.index("'b_key'"), "keys are sorted, so a regenerated file diffs by key"
    assert "a_key: { en: 'It\\'s\\nhere', 'zh-Hans': '在这里', 'zh-Hant': '在這裡', ja: 'ここ', ko: '여기' }," in bundle
    assert render_teams_text(locales, {}).count("TeamsKey = never;") == 1

def test_teams_bundle_rejects_a_reference_without_a_complete_plain_text():
    import pytest
    complete = {locale: "x" for locale in TEAMS_TEXT_LOCALES}
    cases = {
        "is not in en.json": (_teams_locales(), "missing_key"),
        "has no plain text in ja.json": (_teams_locales(partial_key={**complete, "ja": ""}), "partial_key"),
        "has no plain text in ko.json": (_teams_locales(plural_key={**complete, "ko": {"plural": {"other": "x"}}}), "plural_key"),
        "uses an iOS placeholder in zh-Hant.json": (_teams_locales(mobile_key={**complete, "zh-Hant": "%@ 位"}), "mobile_key"),
    }
    for message, (locales, key) in cases.items():
        with pytest.raises(ValueError) as error:
            render_teams_text(locales, {key: ["Host.tsx:7"]})
        assert message in str(error.value) and key in str(error.value) and "Host.tsx:7" in str(error.value)
    assert "`_fmt`" in str(error.value), "the error says what to reference instead of a mobile key"

def test_teams_bundle_freshness_is_checked(tmp_path):
    locales = _teams_locales(teams_title={locale: locale for locale in TEAMS_TEXT_LOCALES})
    root = _teams_sources(tmp_path / "repo", {"packages/teams/View.tsx": "t('teams_title')\n"})
    ios_out, android_out, bundle = tmp_path / "ios", tmp_path / "android", tmp_path / "repo" / "packages" / "teams" / "i18n.generated.ts"
    write_ios(locales, ios_out)
    write_android(locales, android_out)
    assert write_teams_text(locales, bundle, root) == 1
    assert stale_problems(locales, ios_out, android_out, bundle, root) == []
    bundle.write_text(bundle.read_text(encoding="utf-8").replace("ja: 'ja'", "ja: 'ジャ'"), encoding="utf-8")
    assert any("i18n.generated.ts" in p and "out of date" in p for p in stale_problems(locales, ios_out, android_out, bundle, root))
    (root / "packages" / "teams" / "View.tsx").write_text("t('teams_title'); t('teams_title');\n", encoding="utf-8")
    write_teams_text(locales, bundle, root)
    assert stale_problems(locales, ios_out, android_out, bundle, root) == [], "a second reference to a key does not change the bundle"
    bundle.unlink()
    assert any("i18n.generated.ts" in p and "missing" in p for p in stale_problems(locales, ios_out, android_out, bundle, root))

def test_teams_out_names_where_the_bundle_goes_and_custom_directories_leave_the_committed_one_alone(tmp_path):
    committed = Path(__file__).resolve().parent.parent.parent / "packages" / "teams" / "i18n.generated.ts"
    before = committed.read_bytes()
    here = os.path.dirname(__file__)
    rc = subprocess.run([sys.executable, "generate.py", "--ios-out", str(tmp_path / "ios"), "--android-out", str(tmp_path / "android")], cwd=here).returncode
    assert rc == 0 and committed.read_bytes() == before
    elsewhere = tmp_path / "bundle" / "i18n.generated.ts"
    rc = subprocess.run([sys.executable, "generate.py", "--ios-out", str(tmp_path / "ios"), "--android-out", str(tmp_path / "android"), "--teams-out", str(elsewhere)], cwd=here).returncode
    assert rc == 0 and elsewhere.read_bytes() == before, "the committed bundle is what a clean run writes"
    result = subprocess.run([sys.executable, "generate.py", "--check", "--teams-out", str(elsewhere)], cwd=here, capture_output=True, text=True)
    assert result.returncode == 1 and "--check cannot be combined" in result.stderr

if __name__ == "__main__":
    import pytest
    sys.exit(pytest.main(sys.argv))

def test_plural_values_become_ios_variations(tmp_path):
    locales = {
        "zh-Hans": {"chat_asked %lld": "已询问 %lld 个问题"},
        "en": {"chat_asked %lld": {"plural": {"one": "Asked %lld question", "other": "Asked %lld questions"}}},
    }
    write_ios(locales, tmp_path)
    write_android(locales, tmp_path / "android")
    entry = json.load(open(tmp_path / "Localizable.xcstrings"))["strings"]["chat_asked %lld"]["localizations"]
    assert entry["zh-Hans"] == {"stringUnit": {"state": "translated", "value": "已询问 %lld 个问题"}}
    assert entry["en"]["variations"]["plural"]["one"]["stringUnit"]["value"] == "Asked %lld question"
    assert entry["en"]["variations"]["plural"]["other"]["stringUnit"]["value"] == "Asked %lld questions"

def test_plural_values_are_rejected_for_android_keys(tmp_path):
    import pytest
    locales = {"zh-Hans": {"chat_asked": {"plural": {"other": "%d"}}}}
    with pytest.raises(ValueError):
        write_android(locales, tmp_path)
