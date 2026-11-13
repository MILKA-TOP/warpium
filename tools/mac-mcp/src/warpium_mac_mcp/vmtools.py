"""Small helpers used by the Tart VM scripts in ``vm/`` (run on the host).

``python -m warpium_mac_mcp.vmtools kcpassword`` reads a password on stdin and
prints the base64 of the matching ``/etc/kcpassword`` file, which macOS uses
for automatic login. The guest needs auto-login so the MCP server has a GUI
(window server) session; when setup.sh replaces the image's default password
(opt-in, WARPIUM_VM_CHANGE_PASSWORD=1) it has to rewrite this file too.
"""

from __future__ import annotations

import base64
import sys

# Fixed XOR key macOS uses to obfuscate /etc/kcpassword (it is not encryption).
_KC_KEY = bytes([0x7D, 0x89, 0x52, 0x23, 0xD2, 0xBC, 0xDD, 0xEA, 0xA3, 0xB9, 0x1F])
_KC_BLOCK = 12


def kcpassword(password: str) -> bytes:
    """Encodes ``password`` in the /etc/kcpassword format: the UTF-8 bytes plus
    a NUL terminator, zero-padded to a multiple of 12, XORed with the key."""
    raw = password.encode("utf-8")
    if not raw or b"\x00" in raw:
        raise ValueError("password must be non-empty and contain no NUL")
    raw += b"\x00"
    raw += b"\x00" * (-len(raw) % _KC_BLOCK)
    return bytes(b ^ _KC_KEY[i % len(_KC_KEY)] for i, b in enumerate(raw))


def main(argv: list[str]) -> int:
    if argv != ["kcpassword"]:
        print("usage: python -m warpium_mac_mcp.vmtools kcpassword < password", file=sys.stderr)
        return 2
    password = sys.stdin.readline().rstrip("\n")
    print(base64.b64encode(kcpassword(password)).decode("ascii"))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
