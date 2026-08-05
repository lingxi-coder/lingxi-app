import json, subprocess, tempfile, os, sys
sys.path.insert(0, os.path.dirname(__file__))
from generate import load_locales, validate_consistency

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

def test_generate_idempotent(tmp_path):
    out = tmp_path / "out"
    rc = subprocess.run([sys.executable, "generate.py", "--ios-out", str(out), "--android-out", str(out), "--check"], cwd=os.path.dirname(__file__)).returncode
    assert rc == 0

if __name__ == "__main__":
    import pytest
    sys.exit(pytest.main(sys.argv))
