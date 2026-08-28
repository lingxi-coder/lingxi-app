import json, subprocess, tempfile, os, sys
sys.path.insert(0, os.path.dirname(__file__))
from generate import load_locales, validate_consistency, write_android, write_ios, stale_problems

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

def test_generate_check_exit_0(tmp_path):
    out = tmp_path / "out"
    result = subprocess.run([sys.executable, "generate.py", "--ios-out", str(out), "--android-out", str(out), "--check"], cwd=os.path.dirname(__file__), capture_output=True, text=True)
    assert result.returncode == 0
    assert "OK:" in result.stdout

def test_ios_catalog_shape(tmp_path):
    rc = subprocess.run([sys.executable, "generate.py", "--ios-out", str(tmp_path)], cwd=os.path.dirname(__file__)).returncode
    assert rc == 0
    cat = json.load(open(tmp_path / "Localizable.xcstrings"))
    assert cat["sourceLanguage"] == "zh-Hans"
    assert cat["version"] == "1.0"
    assert "app_name" in cat["strings"]
    # InfoPlist keys present under their plist names
    assert "NSCameraUsageDescription" in cat["strings"]
    en = cat["strings"]["app_name"]["localizations"]["en"]["stringUnit"]["value"]
    assert en == "Lingxi"

def test_android_resources(tmp_path):
    rc = subprocess.run([sys.executable, "generate.py", "--android-out", str(tmp_path)], cwd=os.path.dirname(__file__)).returncode
    assert rc == 0
    xml = open(tmp_path / "values-en" / "strings.xml").read()
    assert '<string name="app_name">Lingxi</string>' in xml

def test_bundled_local_app_descriptions_are_canonical_and_generated(tmp_path):
    locales = load_locales(os.path.dirname(__file__))
    keys = [
        "settings_skill_bundled_ionic_react_local_app_desc",
        "settings_skill_bundled_canvas_2d_local_app_desc",
        "settings_skill_bundled_threejs_local_app_desc",
    ]
    for locale in ["zh-Hans", "zh-Hant", "en", "ja", "ko"]:
        for key in keys:
            assert locales[locale][key]

    ios_out = tmp_path / "ios"
    android_out = tmp_path / "android"
    write_ios(locales, ios_out)
    write_android(locales, android_out)

    catalog = json.loads((ios_out / "Localizable.xcstrings").read_text(encoding="utf-8"))
    for key in keys:
        assert set(catalog["strings"][key]["localizations"]) == {"zh-Hans", "zh-Hant", "en", "ja", "ko"}

    for values_dir in ["values", "values-zh-rTW", "values-en", "values-ja", "values-ko"]:
        xml = (android_out / values_dir / "strings.xml").read_text(encoding="utf-8")
        for key in keys:
            assert f'<string name="{key}">' in xml

def test_android_escaping(tmp_path):
    locales = {
        "zh-Hans": {
            "k": "It's 5% sure",
            "n": "共 %1$d 个",
            "q": 'a "b"',
            "a": "a <b> & c",
            "p": "100%x",
            "nl": "line one\nline two",
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

if __name__ == "__main__":
    import pytest
    sys.exit(pytest.main(sys.argv))
