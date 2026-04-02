#!/usr/bin/env bash
# Restart Playwright MCP server for VS Code Copilot
#
# Usage: ./workflow/restart_playwright_mcp.sh
#
# This kills any stale Playwright MCP processes so VS Code's
# auto-start mechanism can relaunch a fresh one on next tool call.
# After running this, the next Playwright MCP tool invocation in
# Copilot will spin up a new server automatically.

set -euo pipefail

echo "=== Playwright MCP Restart ==="

# Kill any stale playwright MCP processes
PIDS=$(pgrep -f "@playwright/mcp" 2>/dev/null || true)
if [ -n "$PIDS" ]; then
  echo "Killing stale Playwright MCP processes: $PIDS"
  kill $PIDS 2>/dev/null || true
  sleep 1
  # Force kill if still alive
  kill -9 $PIDS 2>/dev/null || true
  echo "Done."
else
  echo "No stale Playwright MCP processes found."
fi

# Also kill any orphaned chromium/headless_shell from Playwright
CHROME_PIDS=$(pgrep -f "chromium.*--remote-debugging" 2>/dev/null || true)
if [ -n "$CHROME_PIDS" ]; then
  echo "Killing orphaned Playwright browser processes: $CHROME_PIDS"
  kill $CHROME_PIDS 2>/dev/null || true
  sleep 1
  kill -9 $CHROME_PIDS 2>/dev/null || true
  echo "Done."
else
  echo "No orphaned browser processes found."
fi

echo ""
echo "Playwright MCP will auto-start on next tool call."
echo "If it still fails, run 'MCP: List Servers' from VS Code command palette"
echo "and click restart on 'microsoft/playwright-mcp'."
