# freminal-shell-integration v2
#
# Freminal zsh `.zshenv` — loaded automatically when Freminal spawns zsh.
#
# Freminal launches zsh with `ZDOTDIR=<this dir>` so this `.zshenv` is
# sourced unconditionally as zsh's first startup file.  See
# `freminal-terminal-emulator/src/io/pty.rs::run_terminal` for the spawn
# site and Documents/DESIGN_DECISIONS.md ("Shell Integration Architecture")
# for the rationale.
#
# This file is overwritten on every freminal launch — do NOT edit it.
# To opt out of shell-integration injection, set
# `[shell_integration] set_term_program = false` in ~/.config/freminal/config.toml.

# Step 1: restore the user's original ZDOTDIR so the rest of zsh's startup
# files (.zshrc, .zlogin, etc.) are loaded from their normal location.
#
# Freminal stashes the user's pre-existing ZDOTDIR (if any) into the sentinel
# variable `__FREMINAL_ZSH_ZDOTDIR` before invoking zsh.  We restore it here
# and then chain to the user's real .zshenv if present.
#
# The `(+)` parameter-test syntax distinguishes "set to empty" from "unset",
# matching the precision Ghostty uses in its own integration.
if (( ${+__FREMINAL_ZSH_ZDOTDIR} )); then
	ZDOTDIR="${__FREMINAL_ZSH_ZDOTDIR}"
	unset __FREMINAL_ZSH_ZDOTDIR
else
	unset ZDOTDIR
fi

# Step 2: source the user's real .zshenv if it exists.  Errors are silenced
# so a broken user file does not abort our hook install.
__freminal_user_zdotdir="${ZDOTDIR:-$HOME}"
if [ -f "${__freminal_user_zdotdir}/.zshenv" ]; then
	# shellcheck disable=SC1091
	source "${__freminal_user_zdotdir}/.zshenv" 2>/dev/null
fi

# Step 3: source our integration script.  We locate it relative to the
# directory that contained this .zshenv — that is the freminal resources
# zsh/ directory, which always also contains `freminal-integration`.
#
# `${(%):-%N}` expands to the name of the currently-executing script in
# zsh.  We dirname it to get our own directory.
__freminal_self_dir="${${(%):-%N}:A:h}"
if [ -f "${__freminal_self_dir}/freminal-integration" ]; then
	# shellcheck disable=SC1091
	source "${__freminal_self_dir}/freminal-integration"
fi

unset __freminal_user_zdotdir __freminal_self_dir
