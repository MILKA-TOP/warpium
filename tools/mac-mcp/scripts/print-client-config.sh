#!/bin/sh
# Run on the Mac host. Prints the `claude mcp add ...` command (and an
# equivalent .mcp.json snippet) to paste into the Claude Code container.
# The output contains the bearer token: don't commit it or paste it anywhere
# public.
set -eu
cd "$(dirname "$0")/.."
exec uv run warpium-mac-mcp client-config "$@"
