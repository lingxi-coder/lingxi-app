import json, subprocess, tempfile, os, sys
sys.path.insert(0, os.path.dirname(__file__))
from generate import load_locales, validate_consistency, write_android

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

def test_android_escaping(tmp_path):
    locales = {
        "zh-Hans": {"k": "It's 5% sure", "n": "共 %1$d 个"},
        "en": {"k": "x", "n": "y"},
    }
    write_android(locales, tmp_path)
    xml = open(tmp_path / "values" / "strings.xml").read()
    assert '<string name="k">It\\\'s 5%% sure</string>' in xml
    assert '<string name="n">共 %1$d 个</string>' in xml

if __name__ == "__main__":
    import pytest
    sys.exit(pytest.main(sys.argv))
