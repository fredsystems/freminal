# shellcheck shell=bash
# Freminal shell integration for bash.  This file is intended to be SOURCED,
# not executed — there is intentionally no shebang.
#
# Emits OSC 133 (FinalTerm/FTCS) prompt + command markers and OSC 7 cwd
# updates so Freminal can render command blocks, exit-status gutters,
# and command-aware navigation.
#
# Source from your ~/.bashrc:
#
#   if [ -f ~/.config/freminal/shell-integration/freminal.bash ]; then
#       . ~/.config/freminal/shell-integration/freminal.bash
#   fi
#
# This script is a no-op outside Freminal (it checks $TERM_PROGRAM).
# It is idempotent — sourcing twice has no extra effect.

# Guard: only run inside Freminal.
[ "${TERM_PROGRAM:-}" = "freminal" ] || return 0

# Guard: only install hooks once per shell session.
[ -n "${__FREMINAL_SHELL_INTEGRATION_LOADED:-}" ] && return 0
__FREMINAL_SHELL_INTEGRATION_LOADED=1

# ── Marker constants ──────────────────────────────────────────────────────────
# These are the raw escape sequences.  Inside PS1 they must be wrapped with
# \[...\] so bash knows they are zero-width (no cursor movement).  Outside PS1
# they are printed directly with printf.
__FREMINAL_OSC_A='\033]133;A\007'
__FREMINAL_OSC_B='\033]133;B\007'
__FREMINAL_OSC_C='\033]133;C\007'
# D and OSC 7 are built dynamically (carry exit code / cwd).

# ── PROMPT_COMMAND hook ───────────────────────────────────────────────────────
# Runs just before bash draws the next prompt.  We capture $? at the very top
# because any subsequent command inside this function would overwrite it.
__freminal_prompt_command() {
	local __freminal_exit=$?

	# Emit D with the exit code of the just-completed command.
	printf '\033]133;D;%s\007' "${__freminal_exit}"

	# Emit OSC 7 cwd update.
	local __freminal_hostname
	__freminal_hostname="$(hostname 2>/dev/null || echo localhost)"
	printf '\033]7;file://%s%s\007' "${__freminal_hostname}" "${PWD}"
}

# Append our hook to PROMPT_COMMAND rather than replacing it.
# Handle the case where PROMPT_COMMAND is unset, a string, or an array.
if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" =~ "declare -a" ]]; then
	# It's already an array.
	PROMPT_COMMAND+=(__freminal_prompt_command)
else
	# It's a string (or unset).  Convert to an append pattern.
	if [ -n "${PROMPT_COMMAND:-}" ]; then
		PROMPT_COMMAND="${PROMPT_COMMAND};__freminal_prompt_command"
	else
		PROMPT_COMMAND="__freminal_prompt_command"
	fi
fi

# ── PS1 wrapping ──────────────────────────────────────────────────────────────
# Wrap the user's existing PS1 with the A marker (before prompt) and the B
# marker (after prompt, where the user types).  \[...\] tells readline these
# sequences are zero-width so line editing works correctly.
#
# We save the original PS1 exactly once.  If PS1 is empty or unset we fall
# back to a minimal default so the markers still bracket something visible.
if [ -z "${__FREMINAL_ORIGINAL_PS1+x}" ]; then
	__FREMINAL_ORIGINAL_PS1="${PS1:-\\$ }"
	PS1='\[\033]133;A\007\]'"${__FREMINAL_ORIGINAL_PS1}"'\[\033]133;B\007\]'
fi

# ── DEBUG trap (C marker) ─────────────────────────────────────────────────────
# The DEBUG trap fires before every simple command.  We must be careful to emit
# C only once per real user command, not for every internal expansion.
#
# Conditions where we must NOT emit C:
#   1. During tab-completion ($COMP_LINE is set).
#   2. When $BASH_COMMAND is empty (shouldn't happen but guard anyway).
#   3. When the command is our own prompt_command hook (avoid recursion).
#   4. First call after a prompt: bash fires DEBUG for the command the user
#      actually typed, which is what we want.  We use a state flag to skip
#      the spurious DEBUG fires that happen inside PROMPT_COMMAND itself.
#
# The __FREMINAL_CMD_PENDING flag is set by the DEBUG trap and cleared by
# PROMPT_COMMAND.  This prevents double-emission when bash fires DEBUG for
# sub-commands inside PROMPT_COMMAND.
__freminal_debug_trap() {
	# Skip during tab-completion.
	[ -n "${COMP_LINE+x}" ] && return 0
	# Skip if no command text.
	[ -z "${BASH_COMMAND:-}" ] && return 0
	# Skip our own internal functions.
	case "${BASH_COMMAND}" in
	__freminal_*) return 0 ;;
	esac

	printf '%b' "${__FREMINAL_OSC_C}"
}

# Install the DEBUG trap, composing with any existing trap.
__freminal_existing_debug_trap="$(trap -p DEBUG 2>/dev/null)"
if [ -n "${__freminal_existing_debug_trap}" ]; then
	# There is an existing DEBUG trap.  Prepend our emission.
	# Extract just the command string from "trap -- 'cmd' DEBUG".
	__freminal_existing_debug_cmd="${__freminal_existing_debug_trap#trap -- \'}"
	__freminal_existing_debug_cmd="${__freminal_existing_debug_cmd%\' DEBUG}"
	# shellcheck disable=SC2064
	trap "__freminal_debug_trap; ${__freminal_existing_debug_cmd}" DEBUG
else
	trap '__freminal_debug_trap' DEBUG
fi
unset __freminal_existing_debug_trap __freminal_existing_debug_cmd
