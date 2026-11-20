# freminal-shell-integration v4
# shellcheck shell=bash
#
# Freminal bash integration — loaded automatically when Freminal spawns bash.
#
# Freminal launches bash with `--posix` and `ENV=<this file>` so this script
# is sourced unconditionally regardless of how the user normally invokes
# bash.  See `freminal-terminal-emulator/src/io/pty.rs::run_terminal` for
# the spawn site and Documents/DESIGN_DECISIONS.md ("Shell Integration
# Architecture") for the rationale.
#
# Emits OSC 133 (FinalTerm/FTCS) prompt + command markers and OSC 7 cwd
# updates so Freminal can render command blocks, exit-status gutters, and
# command-aware navigation.  Every emitted marker carries
# `freminal=1;fid=$$-<N>` where N is a per-prompt counter.
#
# This file is overwritten on every freminal launch — do NOT edit it.
# To opt out of shell-integration injection, set
# `[shell_integration] set_term_program = false` in ~/.config/freminal/config.toml.

# Step 1: cancel POSIX mode so the user's interactive features (history
# expansion, aliases, etc.) work normally.  Freminal launched bash with
# --posix only so it would honour $ENV; we don't actually want POSIX
# semantics for the user's shell session.
set +o posix

# Guard: only run under Freminal.  Freminal sets TERM_PROGRAM=freminal at
# spawn (when [shell_integration] set_term_program = true, which is the
# default).  If this script is ever sourced under a different terminal
# (ghostty, wezterm, kitty, iTerm, etc.) — for example because a user
# manually sourced the persisted copy in ~/.local/share/freminal — we must
# not install hooks or emit OSC 133 sequences, since those terminals'
# parsers may treat unrecognised tokens as errors.  Under normal use this
# file is only sourced by freminal's own bash spawn (via `bash --posix`
# + `ENV=`), so this guard is purely defensive.
if [ "${TERM_PROGRAM:-}" != "freminal" ]; then
	# `return` works when this file is sourced (the normal path) and fails
	# (silenced via redirect) if it was ever exec'd directly; `exit` is the
	# fallback for the exec'd case.  shellcheck cannot statically prove the
	# || branch is reachable.
	# shellcheck disable=SC2317
	return 0 2>/dev/null || exit 0
fi

# Step 2: chain to the user's normal bash startup.  We mimic bash's own
# precedence:
#   - login shell: ~/.bash_profile, then ~/.bash_login, then ~/.profile
#   - interactive non-login: ~/.bashrc
# Errors are silenced so a broken rc file does not abort our hook install.
if shopt -q login_shell; then
	if [ -f "$HOME/.bash_profile" ]; then
		# shellcheck disable=SC1091
		. "$HOME/.bash_profile" 2>/dev/null
	elif [ -f "$HOME/.bash_login" ]; then
		# shellcheck disable=SC1091
		. "$HOME/.bash_login" 2>/dev/null
	elif [ -f "$HOME/.profile" ]; then
		# shellcheck disable=SC1091
		. "$HOME/.profile" 2>/dev/null
	fi
else
	if [ -f "$HOME/.bashrc" ]; then
		# shellcheck disable=SC1091
		. "$HOME/.bashrc" 2>/dev/null
	fi
fi

# Guard: only install hooks once per shell session, even if this file is
# sourced again (e.g. by `exec bash`).
if [ -n "${__FREMINAL_SHELL_INTEGRATION_LOADED:-}" ]; then
	# Same dual-mode return as above; `true` is a harmless no-op fallback.
	# shellcheck disable=SC2317
	return 0 2>/dev/null || true
fi
__FREMINAL_SHELL_INTEGRATION_LOADED=1

# ── OSC 1338 HISTFILE report (Task 72.15) ────────────────────────────────────
# Report the shell-evaluated $HISTFILE so freminal can seed the Quick
# Command History Palette with the file the shell will actually read,
# rather than the parent-environment value (which may be unset or stale
# if the user sets HISTFILE inside .bashrc as a shell variable rather
# than exporting it).
#
# The path is sent verbatim — freminal trims trailing whitespace and
# tolerates spaces in paths.  Empty $HISTFILE is suppressed: if it is
# unset, the env-derived loader's default (~/.bash_history) is already
# the right answer.
if [ -n "${HISTFILE:-}" ]; then
	printf '\033]1338;HISTFILE=%s\007' "${HISTFILE}"
fi

# ── sudo keeps TERMINFO (Task 151.12; freminal's own wrapper, Task 151.C5) ───
# sudo's environment reset drops TERMINFO, so programs run under sudo lose
# freminal's terminfo entry.  When $FREMINAL_SHELL_FEATURES (a comma-separated
# list) names the item `sudo` and TERMINFO is set, `sudo` becomes a function
# that runs the real sudo with `--preserve-env=TERMINFO` first.  An edit
# invocation (`-e`, `--edit`, including grouped or after other options) is
# passed through unchanged, because sudo refuses `--preserve-env` with an edit.
# This is freminal's own implementation, written from sudo(8)'s option syntax.
if [[ ",${FREMINAL_SHELL_FEATURES:-}," == *",sudo,"* && -n "${TERMINFO:-}" ]]; then
	# Resolves a long-option name (without the leading dashes) the way sudo
	# does: an exact name wins, otherwise a prefix of exactly one name.  The
	# full name, or nothing for an unknown or ambiguous one, is left in
	# __freminal_sudo_long.
	__freminal_sudo_long_name() {
		local candidate found=""
		local matches=0
		__freminal_sudo_long=""
		for candidate in askpass auth-type background bell chdir chroot close-from command-timeout edit group help host list login login-class no-update non-interactive other-user preserve-env preserve-groups prompt remove-timestamp reset-timestamp role set-home shell stdin type user validate version; do
			if [[ "$candidate" == "$1" ]]; then
				__freminal_sudo_long="$candidate"
				return 0
			fi
			if [[ "$candidate" == "$1"* ]]; then
				matches=$((matches + 1))
				found="$candidate"
			fi
		done
		if [[ "$matches" -eq 1 ]]; then
			__freminal_sudo_long="$found"
		fi
	}

	# Succeeds when the sudo arguments request an edit before the command word.
	__freminal_sudo_is_edit() {
		local word rest letter name
		local takes_value="no"
		for word in "$@"; do
			if [[ "$takes_value" == "yes" ]]; then
				takes_value="no"
				continue
			fi
			if [[ "$takes_value" == "host" ]]; then
				# After a standalone `-h`: the next word is the host unless it is an
				# option or a NAME=value assignment (a path-like word is a host).
				# `[=]*` rather than `=*`, which zsh would try to expand as `=command`.
				takes_value="no"
				case "$word" in
				-*) ;;
				/* | [=]*) continue ;;
				*=*) ;;
				*) continue ;;
				esac
			fi
			case "$word" in
			--)
				return 1
				;;
			--*)
				name="${word#--}"
				name="${name%%=*}"
				__freminal_sudo_long_name "$name"
				case "$__freminal_sudo_long" in
				edit)
					return 0
					;;
				auth-type | chdir | chroot | close-from | command-timeout | group | host | login-class | other-user | prompt | role | type | user)
					# Takes its value from the next word unless given as --name=value.
					if [[ "$word" != *=* ]]; then
						takes_value="yes"
					fi
					;;
				esac
				;;
			-?*)
				rest="${word#-}"
				while [[ -n "$rest" ]]; do
					letter="${rest:0:1}"
					rest="${rest:1}"
					case "$letter" in
					e)
						return 0
						;;
					[aCcDgpRrTtUu])
						# The value is the rest of the word, or the next word.
						if [[ -z "$rest" ]]; then
							takes_value="yes"
						fi
						break
						;;
					h)
						# An optional value: the rest of this word; a standalone `-h`
						# also takes the next word as the host (see above).
						if [[ "$word" == "-h" ]]; then
							takes_value="host"
						fi
						break
						;;
					esac
				done
				;;
			*=*) ;;
			*)
				return 1
				;;
			esac
		done
		return 1
	}

	function sudo {
		if __freminal_sudo_is_edit "$@"; then
			command sudo "$@"
		else
			command sudo --preserve-env=TERMINFO "$@"
		fi
	}
fi

# Per-command counter used to give each command lifecycle a unique `fid`
# (A→B→C→D all share one fid; the next command gets a fresh one).
#
# Why split into `next` + a stored payload, rather than echoing from a
# function?  The natural `$(__freminal_fid_payload)` invocation runs in a
# subshell and cannot mutate parent state — so the counter would reset to
# 0 on every call.  Instead, the parent shell calls `__freminal_fid_next`
# as a plain command (no subshell) to roll the counter forward at command
# boundaries, and any consumer reads `${__FREMINAL_FID_PAYLOAD}` directly.
#
# The fid identifies a *command lifecycle*, not an individual marker
# emission.  All four FTCS markers for the same command (A from PS1
# start, B from PS1 end, C from the DEBUG trap, D from the next
# PROMPT_COMMAND) carry the same fid; the buffer keys the start/output/end
# rows on that fid to stitch the block back together.  Nested or
# concurrent shells get unique fids via the `$$` prefix (different PIDs).
__FREMINAL_FID_COUNTER=0
__FREMINAL_FID_PAYLOAD="freminal=1;fid=$$-0"

# Roll the counter forward and refresh __FREMINAL_FID_PAYLOAD.  Must be
# called as a plain command (not inside `$(…)`) so the assignments stick.
__freminal_fid_next() {
	__FREMINAL_FID_COUNTER=$((__FREMINAL_FID_COUNTER + 1))
	__FREMINAL_FID_PAYLOAD="freminal=1;fid=$$-${__FREMINAL_FID_COUNTER}"
}

# ── PROMPT_COMMAND hook (D marker + OSC 7 cwd + PS1 re-wrap for A and B) ─────
# Runs just before bash draws the next prompt.  We capture $? at the very
# top because any subsequent command would overwrite it.
#
# We deliberately re-wrap PS1 every cycle (rather than once at startup)
# because prompt frameworks like oh-my-posh, Starship, and Powerline-shell
# overwrite PS1 from their own PROMPT_COMMAND entries.  By re-wrapping every
# cycle from a hook that's appended to PROMPT_COMMAND (and re-armed to the
# end), we guarantee our A/B wrap survives any framework that ran earlier in
# the chain.
__freminal_prompt_command() {
	local __freminal_exit=$?

	# D marker closes the *previous* command, so it uses the current
	# (about-to-be-replaced) fid payload.
	printf '\033]133;D;%s;%s\007' "${__freminal_exit}" "${__FREMINAL_FID_PAYLOAD}"

	# Roll the fid forward for the upcoming command lifecycle.  The new
	# fid is shared by A (prompt start), B (prompt end), C (DEBUG trap
	# when the user runs a command), and the next prompt_command's D.
	__freminal_fid_next

	# Allow the DEBUG trap to emit exactly one C for the upcoming command.
	__FREMINAL_C_EMITTED=0

	# OSC 7 cwd notification.
	local __freminal_hostname
	__freminal_hostname="$(hostname 2>/dev/null || echo localhost)"
	printf '\033]7;file://%s%s\007' "${__freminal_hostname}" "${PWD}"

	# Re-wrap PS1.  Strip any prior wrap first to avoid stacking.
	# `\[...\]` tells readline these sequences are zero-width.
	# `${__FREMINAL_FID_PAYLOAD}` is re-expanded by bash at every prompt
	# redraw (promptvars shopt is on by default), picking up the value
	# set just above (and any subsequent rolls — but no further rolls
	# happen until the next prompt_command).
	__freminal_strip_ps1_wrap
	PS1='\[\033]133;A;${__FREMINAL_FID_PAYLOAD}\007\]'"${PS1}"'\[\033]133;B;${__FREMINAL_FID_PAYLOAD}\007\]'

	__freminal_rearm_prompt_command

	# Re-arm the C hook every cycle.  bash-preexec (Starship et al.) may have
	# loaded after us, or may re-install its DEBUG dispatcher each prompt and
	# clobber our trap; re-running the installer keeps our `preexec_functions`
	# entry / DEBUG trap in place.  Idempotent.
	__freminal_install_c_hook
}

# Strip any existing freminal A/B wrap from PS1 (defensive: avoids stacking
# wraps if prompt_command runs more than once before re-arm).
#
# PS1 stores the markers as a literal string containing the four-character
# sequences `\[`, `\033`, `\007`, `\]` (bash does NOT mutate PS1 — those
# escapes are only interpreted at draw time by readline / promptvars).  We
# therefore need to match those literal characters as a glob pattern in
# `${var//pat/repl}`.
#
# Glob escaping:
#   `\\` matches a single literal `\`
#   `\[` matches a single literal `[`
#   `\]` matches a single literal `]`
# So the pair of characters `\[` in PS1 is matched by the four-char glob
# `\\\[` (literal-backslash + literal-`[`).
__freminal_strip_ps1_wrap() {
	# Single quotes are required: `${__FREMINAL_FID_PAYLOAD}` must be stored
	# as a literal pattern (it appears verbatim in PS1, since PS1 is itself
	# stored with the unexpanded `${…}` reference — bash only expands it at
	# prompt-draw time via promptvars).  Double quotes would interpolate the
	# variable here and break the match.
	# shellcheck disable=SC2016
	local marker_a='\\\[\\033]133;A;${__FREMINAL_FID_PAYLOAD}\\007\\\]'
	# shellcheck disable=SC2016
	local marker_b='\\\[\\033]133;B;${__FREMINAL_FID_PAYLOAD}\\007\\\]'
	PS1="${PS1//${marker_a}/}"
	PS1="${PS1//${marker_b}/}"
}

# If PROMPT_COMMAND is an array (bash 5.1+), move our entry to the end.
# Otherwise it's a string; we can't reliably re-order entries inside a
# string PROMPT_COMMAND, so we just ensure we're present.
__freminal_rearm_prompt_command() {
	if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" =~ "declare -a" ]]; then
		local i new=()
		for i in "${PROMPT_COMMAND[@]}"; do
			[ "$i" = "__freminal_prompt_command" ] || new+=("$i")
		done
		new+=(__freminal_prompt_command)
		PROMPT_COMMAND=("${new[@]}")
	fi
}

# Append our hook to PROMPT_COMMAND rather than replacing it.
if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" =~ "declare -a" ]]; then
	PROMPT_COMMAND+=(__freminal_prompt_command)
else
	if [ -n "${PROMPT_COMMAND:-}" ]; then
		PROMPT_COMMAND="${PROMPT_COMMAND};__freminal_prompt_command"
	else
		PROMPT_COMMAND="__freminal_prompt_command"
	fi
fi

# ── C marker (command execution start) ───────────────────────────────────────
# C must fire once, just before the user's command runs, carrying the fid
# established at the most recent PROMPT_COMMAND (the same fid embedded in the
# A/B markers of the prompt the user just submitted).  freminal computes the
# command's duration from C->D, so a missing C makes the duration fall back
# to the prompt-start time (A->D) — which over-reports by however long the
# user sat at the prompt.
#
# We emit C from a DEBUG trap that we OWN and re-arm every prompt cycle.
#
# Why not bash-preexec's `preexec_functions`?  Freminal launches bash with
# `--posix` (so it honours $ENV to load this file).  Under that launch,
# bash-preexec's interactive-mode gating never enables, so its
# `preexec_functions` dispatch silently never runs — WezTerm's and Starship's
# preexec C markers are dropped too.  A DEBUG trap, by contrast, fires
# natively regardless of bash-preexec's state.
#
# Why re-arm every cycle?  Prompt frameworks (Starship via bash-preexec,
# WezTerm) install their own DEBUG trap on the first prompt, clobbering ours.
# `__freminal_prompt_command` re-arms itself to the END of PROMPT_COMMAND and
# calls `__freminal_install_c_hook` from there, so our re-install runs AFTER
# any framework's install each cycle and composes our handler in front of
# whatever DEBUG trap currently exists.
__freminal_emit_c() {
	printf '\033]133;C;%s\007' "${__FREMINAL_FID_PAYLOAD}"
}

# The DEBUG trap fires before every simple command, so it needs guards:
#   1. During tab-completion ($COMP_LINE is set).
#   2. When $BASH_COMMAND is empty.
#   3. When the command is one of our own internal helpers.
#   4. When the command is part of the prompt machinery (PROMPT_COMMAND,
#      bash-preexec, WezTerm, Starship); those are not user commands.
#   5. Only once per command lifecycle (a compound command / pipeline fires
#      DEBUG per simple command; we want a single C per submitted command).
__freminal_debug_trap() {
	[ -n "${COMP_LINE+x}" ] && return 0
	[ -z "${BASH_COMMAND:-}" ] && return 0
	case "${BASH_COMMAND}" in
	__freminal_* | __bp_* | __wezterm_* | starship_* | _*) return 0 ;;
	esac
	# Emit C only once between prompts.  Reset to 0 in PROMPT_COMMAND.
	[ "${__FREMINAL_C_EMITTED:-0}" = "1" ] && return 0
	__FREMINAL_C_EMITTED=1
	__freminal_emit_c
}

# Install (or re-install) our DEBUG trap, composing IN FRONT of any existing
# DEBUG trap (e.g. bash-preexec's dispatcher) so both run.  Idempotent: if our
# handler is already the leading command, do nothing.  Re-armed every prompt
# cycle from `__freminal_prompt_command`.
__freminal_install_c_hook() {
	local __freminal_existing
	__freminal_existing="$(trap -p DEBUG 2>/dev/null)"
	case "${__freminal_existing}" in
	"trap -- '__freminal_debug_trap"*) return 0 ;; # already leading
	esac
	if [ -n "${__freminal_existing}" ]; then
		# Strip `trap -- '` prefix and `' DEBUG` suffix to recover the command.
		local __freminal_cmd="${__freminal_existing#trap -- \'}"
		__freminal_cmd="${__freminal_cmd%\' DEBUG}"
		# shellcheck disable=SC2064
		trap "__freminal_debug_trap; ${__freminal_cmd}" DEBUG
	else
		trap '__freminal_debug_trap' DEBUG
	fi
}

__freminal_install_c_hook
