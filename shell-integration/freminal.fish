# Freminal shell integration for fish.  This file is intended to be SOURCED,
# not executed — there is intentionally no shebang.
#
# Emits OSC 133 (FinalTerm/FTCS) prompt + command markers and OSC 7 cwd
# updates so Freminal can render command blocks, exit-status gutters,
# and command-aware navigation.
#
# Source from your ~/.config/fish/config.fish:
#
#   if test -f ~/.config/freminal/shell-integration/freminal.fish
#       source ~/.config/freminal/shell-integration/freminal.fish
#   end
#
# This script is a no-op outside Freminal (it checks $TERM_PROGRAM).
# It is idempotent — sourcing twice has no extra effect.

# Guard: only run inside Freminal.
if test "$TERM_PROGRAM" != "freminal"
    exit 0
end

# Guard: only install hooks once per shell session.
if set -q __freminal_shell_integration_loaded
    exit 0
end
set -g __freminal_shell_integration_loaded 1

# ── fish_prompt event (A and B markers) ──────────────────────────────────────
# fish calls all functions listening on fish_prompt before drawing the prompt.
# We emit A before the prompt content and B after it by wrapping the existing
# fish_prompt function if it exists, otherwise installing a thin shim.
#
# fish does not have zero-width prompt escaping like bash/zsh; the markers are
# emitted as plain printf output before and after the prompt function runs.
# The OSC sequences themselves are not visible characters, so this is correct.
function __freminal_fish_prompt --on-event fish_prompt
    printf '\033]133;A\007'
    # fish_prompt is the single function fish calls to draw the prompt.
    # We cannot "wrap" it via on-event (that would cause infinite recursion).
    # Instead we emit A here (before any prompt output) and B immediately
    # after, then let the real fish_prompt (defined by the user or the default)
    # run on its own.  Because on-event handlers run BEFORE the fish_prompt
    # function body, A is emitted first.  B is emitted here too, right after A,
    # which means our B arrives before the user's prompt text.
    #
    # NOTE: A more precise placement of B (after the prompt text) would require
    # overriding fish_prompt itself, which conflicts with user themes (Tide,
    # Starship, etc.).  The approach used here — A then B before the visual
    # prompt — is compatible with all themes and still allows Freminal to
    # identify the prompt region.  The user's typed input follows B regardless.
    printf '\033]133;B\007'
end

# ── fish_preexec event (C marker) ─────────────────────────────────────────────
# fish_preexec fires just before the command is executed.
function __freminal_fish_preexec --on-event fish_preexec
    printf '\033]133;C\007'
end

# ── fish_postexec event (D marker + OSC 7 cwd) ────────────────────────────────
# fish_postexec fires just after the command completes.  The exit status of the
# completed command is available as $status at this point in fish.
# Note: the argument $argv[1] is the command string; $argv[2] is the exit code
# as a string.  We use the $status variable for reliability.
function __freminal_fish_postexec --on-event fish_postexec
    # $argv[2] is the exit code passed by fish to postexec handlers.
    set -l __freminal_exit $argv[2]

    # Emit D with exit code.
    printf '\033]133;D;%s\007' "$__freminal_exit"

    # Emit OSC 7 cwd update.
    set -l __freminal_hostname (hostname 2>/dev/null; or echo localhost)
    printf '\033]7;file://%s%s\007' "$__freminal_hostname" "$PWD"
end
