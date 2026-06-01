import os
import stat
import sys

import lingxi_cli_bin
import pytest


def _install_fake_binary(tmp_path, monkeypatch, script):
    pkg = tmp_path / "lingxi_cli_bin"
    (pkg / "bin").mkdir(parents=True)
    (pkg / "lingxi-package.json").write_text("{}")
    exe = pkg / "bin" / ("lingxi.exe" if os.name == "nt" else "lingxi")
    exe.write_text(script)
    exe.chmod(exe.stat().st_mode | stat.S_IEXEC)
    monkeypatch.setattr(lingxi_cli_bin, "__file__", str(pkg / "__init__.py"))
    return exe


def test_bundled_path_found(tmp_path, monkeypatch):
    _install_fake_binary(tmp_path, monkeypatch, "#!/bin/sh\nexit 0\n")
    assert lingxi_cli_bin.bundled_lingxi_path().is_file()


@pytest.mark.skipif(os.name == "nt", reason="POSIX shell stub")
def test_main_mirrors_exit_code(tmp_path, monkeypatch):
    _install_fake_binary(tmp_path, monkeypatch, "#!/bin/sh\nexit 7\n")
    monkeypatch.setattr(sys, "argv", ["lingxi"])
    assert lingxi_cli_bin.main() == 7


def test_missing_binary_raises(tmp_path, monkeypatch):
    pkg = tmp_path / "lingxi_cli_bin"
    pkg.mkdir()
    (pkg / "lingxi-package.json").write_text("{}")
    monkeypatch.setattr(lingxi_cli_bin, "__file__", str(pkg / "__init__.py"))
    with pytest.raises(FileNotFoundError):
        lingxi_cli_bin.bundled_lingxi_path()
