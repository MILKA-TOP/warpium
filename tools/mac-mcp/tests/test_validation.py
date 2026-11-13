import pytest

from warpium_mac_mcp.cli import check_bind_host
from warpium_mac_mcp.validation import (
    ValidationError,
    parse_key_combo,
    parse_since,
    validate_app_args,
    validate_bin,
    validate_branch,
    validate_click,
    validate_env,
    validate_features,
    validate_grep,
    validate_profile,
    validate_run_id,
    validate_tail,
    validate_test_name,
)


@pytest.mark.parametrize(
    "b", ["master", "claude/mac-mcp", "fix/open-in-new-tab-cwd", "v1.2.3", "feature_x"]
)
def test_valid_branches(b):
    assert validate_branch(b) == b


@pytest.mark.parametrize(
    "b",
    [
        "",
        "-x",
        "--upload-pack=evil",
        "a..b",
        "a b",
        "a;rm",
        "a/",
        "a.lock",
        "HEAD",
        "a@{1}",
        "../x",
        "a//b",
        "a/.hidden",
        "$(x)",
        "a\nb",
        "x" * 300,
    ],
)
def test_invalid_branches(b):
    with pytest.raises(ValidationError):
        validate_branch(b)


def test_profile_and_bin():
    assert validate_profile("debug") == "debug"
    with pytest.raises(ValidationError):
        validate_profile("--release")
    assert validate_bin("warp-oss") == "warp-oss"
    with pytest.raises(ValidationError):
        validate_bin("warp")


def test_features():
    assert validate_features(["gui", "warp_control_cli", "gui"]) == ["gui", "warp_control_cli"]
    with pytest.raises(ValidationError):
        validate_features("gui")
    with pytest.raises(ValidationError):
        validate_features(["gui,evil"])
    with pytest.raises(ValidationError):
        validate_features(["--config=x"])
    with pytest.raises(ValidationError):
        validate_features(["nope"], known={"gui"})


def test_test_name_and_run_id():
    assert validate_test_name("test_open_new_tab") == "test_open_new_tab"
    for bad in ["", "Test", "a-b", "a b", "--list", "x;y"]:
        with pytest.raises(ValidationError):
            validate_test_name(bad)
    assert validate_run_id(123) == "123"
    for bad in ["12a", "-1", "", "1 2"]:
        with pytest.raises(ValidationError):
            validate_run_id(bad)


def test_env():
    assert validate_env({"RUST_LOG": "warp=debug", "RUST_BACKTRACE": "1"}) == {
        "RUST_LOG": "warp=debug",
        "RUST_BACKTRACE": "1",
    }
    for bad in [
        {"DYLD_INSERT_LIBRARIES": "x"},
        {"PATH": "/tmp"},
        {"HOME": "/"},
        {"LD_PRELOAD": "x"},
        {"warp_x": "1"},
        {"WARP_LOCAL_CONTROL_DISCOVERY_DIR": "/x"},
        {"WARP_SHELL_PATH": "/tmp/evil"},
        {"WARP_RECORDING_PROCESS_PATH": "/tmp/evil"},
        {"WARP_TEST_CHECKOUT_ROOT": "/"},
        {"WARP_MOCK_RECORDER": "1"},
        {"WARP_DATA_PROFILE": ""},
        {"WARP_API_KEY": "secret"},
        {"WARP_FOO": "1"},
        {"RUST_LOG": "a\nb"},
        {"RUST_LOG": "x" * 2000},
    ]:
        with pytest.raises(ValidationError):
            validate_env(bad)
    assert validate_env(None) == {}


def test_app_args():
    assert validate_app_args(["--foo", "--bar=1"]) == ["--foo", "--bar=1"]
    for bad in [
        ["agent"],
        ["--warpctrl"],
        ["--warpctrl=x"],
        ["--x=$(id)"],
        ["--x y"],
        "--foo",
        ["-"],
        ["--x=a b"],
    ]:
        with pytest.raises(ValidationError):
            validate_app_args(bad)


def test_grep_tail():
    assert validate_grep(None) is None
    assert validate_grep("") is None
    assert validate_grep("error") == "error"
    with pytest.raises(ValidationError):
        validate_grep("x" * 500)
    assert validate_tail(10) == 10
    assert validate_tail(10**9) == 5000
    for bad in [0, -1, True, "5"]:
        with pytest.raises(ValidationError):
            validate_tail(bad)


def test_since():
    now = 1_000_000.0
    assert parse_since("90s", now) == now - 90
    assert parse_since("15m", now) == now - 900
    assert parse_since("2h", now) == now - 7200
    assert parse_since(None, now) is None
    assert parse_since("2026-01-01T00:00:00Z", now) == 1767225600.0
    with pytest.raises(ValidationError):
        parse_since("yesterday", now)


def test_key_combos():
    assert parse_key_combo("cmd+t") == (frozenset({"cmd"}), "t")
    assert parse_key_combo("Cmd+Shift+]") == (frozenset({"cmd", "shift"}), "]")
    assert parse_key_combo("enter") == (frozenset(), "enter")
    assert parse_key_combo("option+left") == (frozenset({"alt"}), "left")
    for bad in ["", "cmd+", "+t", "hyper+t", "cmd+unknownkey", "cmd++", "x" * 100]:
        with pytest.raises(ValidationError):
            parse_key_combo(bad)
    assert parse_key_combo("ctrl+c") == (frozenset({"ctrl"}), "c")
    assert parse_key_combo("cmd+q") == (frozenset({"cmd"}), "q")  # quits Warpium only
    assert parse_key_combo("alt+left") == (frozenset({"alt"}), "left")
    assert parse_key_combo("f5") == (frozenset(), "f5")
    assert parse_key_combo("cmd+2") == (frozenset({"cmd"}), "2")  # Warp tab 2
    for system in [
        "cmd+space",
        "alt+space",
        "ctrl+space",
        "cmd+alt+space",
        "cmd+tab",
        "cmd+shift+tab",
        "ctrl+cmd+q",
        "cmd+shift+q",
        "cmd+alt+shift+q",
        "cmd+alt+esc",
        "cmd+alt+shift+escape",
        "cmd+shift+3",
        "cmd+shift+ctrl+4",
        "ctrl+f2",
        "ctrl+f3",
        "ctrl+left",
        "ctrl+up",
        "fn+a",
        "fn+n",
        "cmd+alt+d",
        "cmd+alt+`",
        "f11",
        "f12",
        "ctrl+2",
    ]:
        with pytest.raises(ValidationError):
            parse_key_combo(system)


def test_click_args():
    validate_click("left", 2)
    for b, c in [("x", 1), ("left", 0), ("left", 4), ("left", True)]:
        with pytest.raises(ValidationError):
            validate_click(b, c)


def test_wildcard_bind_refused():
    for h in ["0.0.0.0", "::"]:  # noqa: S104
        with pytest.raises(SystemExit):
            check_bind_host(h)
    with pytest.raises(SystemExit):
        check_bind_host("example.com")
    check_bind_host("127.0.0.1")
    check_bind_host("192.168.65.1")


def test_scrubbed_env_drops_secrets():
    from warpium_mac_mcp.config import scrubbed_env

    env = scrubbed_env(
        {
            "PATH": "/bin",
            "HOME": "/h",
            "LC_ALL": "C",
            "AWS_SECRET_ACCESS_KEY": "s",
            "GH_TOKEN": "t",
            "ANTHROPIC_API_KEY": "k",
            "DYLD_LIBRARY_PATH": "/x",
        }
    )
    assert env == {"PATH": "/bin", "HOME": "/h", "LC_ALL": "C"}
