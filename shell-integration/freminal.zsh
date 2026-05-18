# Freminal shell integration for zsh.  This file is intended to be SOURCED,
# not executed — there is intentionally no shebang.
#
# Emits OSC 133 (FinalTerm/FTCS) prompt + command markers and OSC 7 cwd
# updates so Freminal can render command blocks, exit-status gutters,
# and command-aware navigation.
#
# Source from your ~/.zshrc:
#
#   if [ -f ~/.config/freminal/shell-integration/freminal.zsh ]; then
#       . ~/.config/freminal/shell-integration/freminal.zsh
#   fi
#
# This script is a no-op outside Freminal (it checks $TERM_PROGRAM).
# It is idempotent — sourcing twice has no extra effect.

# Guard: only run inside Freminal.
[[ "${TERM_PROGRAM:-}" == "freminal" ]] || return 0

# Guard: only install hooks once per shell session.
[[ -n "${__FREMINAL_SHELL_INTEGRATION_LOADED:-}" ]] && return 0
typeset -g __FREMINAL_SHELL_INTEGRATION_LOADED=1

# ── precmd hook (D marker + OSC 7 cwd) ───────────────────────────────────────
# precmd runs just before the prompt is drawn.  $? at this point reflects the
# exit status of the last user command (zsh preserves it through precmd_functions
# calls).
__freminal_precmd() {
    local __freminal_exit=$?

    # Emit D with the exit code of the just-completed command.
    printf '\033]133;D;%s\007' "${__freminal_exit}"

    # Emit OSC 7 cwd update.
    local __freminal_hostname
    __freminal_hostname="$(hostname 2>/dev/null || echo localhost)"
    printf '\033]7;file://%s%s\007' "${__freminal_hostname}" "${PWD}"
}

# Append to precmd_functions (zsh standard hook array).
# autoload -Uz add-zsh-hook is the canonical way; use the array directly as a
# fallback that works even without compinit.
if (( ${+functions[add-zsh-hook]} )); then
    add-zsh-hook precmd __freminal_precmd
else
    precmd_functions+=(__freminal_precmd)
fi

# ── preexec hook (C marker) ───────────────────────────────────────────────────
# preexec runs just before each command is executed.  The first argument is the
# command string as typed.
__freminal_preexec() {
    printf '\033]133;C\007'
}

if (( ${+functions[add-zsh-hook]} )); then
    add-zsh-hook preexec __freminal_preexec
else
    preexec_functions+=(__freminal_preexec)
fi

# ── PROMPT wrapping (A and B markers) ────────────────────────────────────────
# Wrap the user's current PROMPT (PS1) with the A marker before and B marker
# after.  %{...%} tells zsh these sequences are zero-width so cursor movement
# is accounted for correctly.
#
# We save the original PROMPT exactly once to ensure idempotency even if
# PROMPT changes between sources.
if [[ -z "${__FREMINAL_ORIGINAL_PROMPT+x}" ]]; then
    typeset -g __FREMINAL_ORIGINAL_PROMPT="${PROMPT:-%%\  }"
    PROMPT="%{\033]133;A\007%}${__FREMINAL_ORIGINAL_PROMPT}%{\033]133;B\007%}"
fi
