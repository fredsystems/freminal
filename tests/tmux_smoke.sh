#!/usr/bin/env bash
# tmux_smoke.sh — Manual integration test for tmux compatibility in Freminal
#
# This script documents the manual verification procedure for tmux running
# inside Freminal. It is NOT an automated test — tmux requires a real PTY and
# interactive verification. Run this inside a Freminal terminal window.
#
# Prerequisites:
#   - Freminal built and running (this script runs inside Freminal)
#   - tmux installed (tmux >= 3.0 recommended)
#
# Usage:
#   ./tests/tmux_smoke.sh
#
# The script performs automated steps where possible and prompts for manual
# verification where visual confirmation is needed.

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

pass_count=0
fail_count=0
skip_count=0

pass() {
	echo -e "  ${GREEN}PASS${NC}: $1"
	((pass_count++))
}

fail() {
	echo -e "  ${RED}FAIL${NC}: $1"
	((fail_count++))
}

skip() {
	echo -e "  ${YELLOW}SKIP${NC}: $1"
	((skip_count++))
}

ask() {
	local prompt="$1"
	echo -en "  ${YELLOW}?${NC} ${prompt} [y/n]: "
	read -r -n 1 answer
	echo
	[[ "$answer" == "y" || "$answer" == "Y" ]]
}

echo "=== Freminal tmux Smoke Test ==="
echo ""

# ── Prerequisite check ────────────────────────────────────────────────

if ! command -v tmux &>/dev/null; then
	echo -e "${RED}ERROR${NC}: tmux is not installed. Install tmux and re-run."
	exit 1
fi

tmux_version=$(tmux -V)
echo "tmux version: ${tmux_version}"
echo ""

# Kill any leftover test session
tmux kill-session -t freminal_test 2>/dev/null || true

# ── Step 1: Start a detached tmux session ─────────────────────────────

echo "Step 1: Create detached tmux session..."
if tmux new-session -d -s freminal_test -x 80 -y 24; then
	pass "tmux new-session -d -s freminal_test succeeded"
else
	fail "tmux new-session failed"
	exit 1
fi

# ── Step 2: Attach to the session ─────────────────────────────────────

echo ""
echo "Step 2: Attach to tmux session."
echo "  This will attach you to the tmux session. Perform steps 3-9 inside it."
echo "  When done, detach with Ctrl+B d and the script will continue."
echo ""
echo "  Press Enter to attach..."
read -r

tmux attach -t freminal_test

echo ""
echo "=== Post-attach verification ==="
echo ""

# ── Step 3-9: Manual verification ─────────────────────────────────────

echo "Step 3: Status bar"
if ask "Did the tmux status bar render correctly at the bottom of the screen?"; then
	pass "Status bar renders correctly"
else
	fail "Status bar did not render correctly"
fi

echo ""
echo "Step 4: New window (Ctrl+B c)"
if ask "Did Ctrl+B c create a new window (window indicator changed in status bar)?"; then
	pass "New window creation works"
else
	fail "New window creation failed"
fi

echo ""
echo "Step 5: Vertical split (Ctrl+B %)"
if ask "Did Ctrl+B % split the pane vertically (vertical border appeared)?"; then
	pass "Vertical pane split works"
else
	fail "Vertical pane split failed"
fi

echo ""
echo "Step 6: Pane navigation (Ctrl+B Arrow)"
if ask "Did Ctrl+B Arrow navigate between panes correctly?"; then
	pass "Pane navigation works"
else
	fail "Pane navigation failed"
fi

echo ""
echo "Step 7: Detach (Ctrl+B d)"
if ask "Did Ctrl+B d detach cleanly (you returned to this script)?"; then
	pass "Detach works"
else
	fail "Detach failed"
fi

echo ""
echo "Step 8: Re-attach verification"
echo "  The script will now re-attach to verify session persistence."
echo "  Check that the layout (split panes, multiple windows) is preserved."
echo "  Detach again with Ctrl+B d."
echo ""
echo "  Press Enter to re-attach..."
read -r

tmux attach -t freminal_test 2>/dev/null || true

echo ""
if ask "Was the session layout preserved after re-attach?"; then
	pass "Session persistence works"
else
	fail "Session persistence failed"
fi

# ── Step 10: Clean up ─────────────────────────────────────────────────

echo ""
echo "Step 10: Cleaning up tmux session..."
if tmux kill-session -t freminal_test 2>/dev/null; then
	pass "tmux kill-session succeeded"
else
	# Session may already be dead if user killed it during testing
	skip "tmux session already ended"
fi

# ── Summary ───────────────────────────────────────────────────────────

echo ""
echo "=== Summary ==="
echo -e "  ${GREEN}Passed${NC}: ${pass_count}"
echo -e "  ${RED}Failed${NC}: ${fail_count}"
echo -e "  ${YELLOW}Skipped${NC}: ${skip_count}"
echo ""

if [[ $fail_count -gt 0 ]]; then
	echo -e "${RED}Some tests failed. See above for details.${NC}"
	exit 1
fi

echo -e "${GREEN}All tests passed!${NC}"

# ── Known Limitations ─────────────────────────────────────────────────
#
# As of subtasks 9.1-9.5:
# - Compound CSI mode sequences (e.g. ESC[?1049;2004h) are now split and
#   handled individually (fixed in 9.1)
# - DECRPM mode query responses are wired for all supported modes (fixed
#   in 9.2)
# - Modified key sequences (Shift/Ctrl/Alt + arrow/function keys) are
#   sent in xterm format (fixed in 9.3/9.4)
# - DEC private DSR (ESC[?6n) is handled and responds with cursor
#   position (fixed in 9.5)
