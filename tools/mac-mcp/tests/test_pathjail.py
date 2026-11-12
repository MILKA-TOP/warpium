import os

import pytest

from warpium_mac_mcp.pathjail import PathEscapeError, PathJail


@pytest.fixture
def jail(tmp_path):
    root = tmp_path / "ws"
    root.mkdir()
    (root / "sub").mkdir()
    (root / "sub" / "f.txt").write_text("x")
    logs = tmp_path / "logs"
    logs.mkdir()
    (logs / "warp-oss.log").write_text("log")
    (logs / "other.log").write_text("secret")
    return PathJail([root], files=[logs / "warp-oss.log"]), root, tmp_path


def test_inside_paths_allowed(jail):
    j, root, _ = jail
    assert j.resolve(root / "sub" / "f.txt") == (root / "sub" / "f.txt").resolve()
    assert j.resolve("sub/f.txt") == (root / "sub" / "f.txt").resolve()
    assert j.resolve(root) == root.resolve()
    # Non-existent leaf inside the root is fine (e.g. an output file).
    assert j.resolve(root / "new" / "file.png").parent == (root / "new").resolve()


def test_dotdot_escape_rejected(jail):
    j, root, tmp = jail
    with pytest.raises(PathEscapeError):
        j.resolve(root / ".." / "logs" / "other.log")
    with pytest.raises(PathEscapeError):
        j.resolve("../logs/other.log")
    with pytest.raises(PathEscapeError):
        j.resolve("/etc/passwd")


def test_prefix_sibling_rejected(tmp_path):
    (tmp_path / "ws").mkdir()
    (tmp_path / "ws-evil").mkdir()
    j = PathJail([tmp_path / "ws"])
    with pytest.raises(PathEscapeError):
        j.resolve(tmp_path / "ws-evil" / "x")


def test_symlink_escape_rejected(jail):
    j, root, tmp = jail
    os.symlink(tmp / "logs", root / "link")
    with pytest.raises(PathEscapeError):
        j.resolve(root / "link" / "other.log")
    os.symlink("/etc/passwd", root / "pw")
    with pytest.raises(PathEscapeError):
        j.resolve(root / "pw")


def test_symlink_inside_root_ok(jail):
    j, root, _ = jail
    os.symlink(root / "sub", root / "alias")
    assert j.resolve(root / "alias" / "f.txt") == (root / "sub" / "f.txt").resolve()


def test_allowed_individual_files(jail):
    j, _, tmp = jail
    assert j.resolve(tmp / "logs" / "warp-oss.log") == (tmp / "logs" / "warp-oss.log").resolve()
    with pytest.raises(PathEscapeError):
        j.resolve(tmp / "logs" / "other.log")


def test_symlink_to_allowed_file_resolves_to_real(jail):
    j, root, tmp = jail
    os.symlink(tmp / "logs" / "warp-oss.log", root / "log-link")
    assert j.resolve(root / "log-link") == (tmp / "logs" / "warp-oss.log").resolve()


def test_nul_byte_rejected(jail):
    j, root, _ = jail
    with pytest.raises(PathEscapeError):
        j.resolve(str(root) + "/a\x00b")


def test_root_itself_symlink(tmp_path):
    real = tmp_path / "real"
    real.mkdir()
    os.symlink(real, tmp_path / "linkroot")
    j = PathJail([tmp_path / "linkroot"])
    assert j.contains(real / "x")
    assert not j.contains(tmp_path / "x")
