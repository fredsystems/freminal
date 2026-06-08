# freminal-shell-integration v4
#
# Freminal fish integration — installed under `vendor_conf.d/` and loaded
# automatically by fish when Freminal prepends our resources directory to
# $XDG_DATA_DIRS.  See `freminal-terminal-emulator/src/io/pty.rs::run_terminal`
# for the spawn site and Documents/DESIGN_DECISIONS.md ("Shell Integration
# Architecture") for the rationale.
#
# Emits OSC 133 (FinalTerm/FTCS) prompt + command markers and OSC 7 cwd
# updates so Freminal can render command blocks, exit-status gutters, and
# command-aware navigation.  Every emitted marker carries
# `freminal=1;fid=$fish_pid-<N>` where N is a per-prompt counter.
#
# This file is overwritten on every freminal launch — do NOT edit it.
# To opt out, set `[shell_integration] set_term_program = false` in
# ~/.config/freminal/config.toml.

# Guards: only run under Freminal, and only once per shell session.
#
# Freminal sets TERM_PROGRAM=freminal at spawn (when
# [shell_integration] set_term_program = true, which is the default).
# Because vendor_conf.d is loaded by every fish session that sees our
# XDG_DATA_DIRS prepend, we must bail under other terminals so we don't
# install hooks or emit OSC 133 sequences they may mis-parse.
#
# IMPORTANT: vendor_conf.d files are sourced into the user's fish
# process, so `exit` here would kill the user's shell — not just skip
# this file.  We use `return` from inside a wrapper function to abort
# script loading without terminating fish.
function __freminal_should_init
    test "$TERM_PROGRAM" = "freminal"; and not set -q __freminal_shell_integration_loaded
end

if __freminal_should_init
    functions -e __freminal_should_init
    set -g __freminal_shell_integration_loaded 1

    # Per-command counter used to give each command lifecycle a unique `fid`
    # (A→B→C→D all share one fid; the next command gets a fresh one).  The fid
    # identifies a *command lifecycle*, not an individual marker emission.
    set -g __freminal_fid_counter 0
    set -g __freminal_fid_payload "freminal=1;fid=$fish_pid-0"

    # Roll the counter forward and refresh __freminal_fid_payload.  Called
    # once per prompt cycle from the fish_prompt event handler.
    function __freminal_fid_next
        set -g __freminal_fid_counter (math $__freminal_fid_counter + 1)
        set -g __freminal_fid_payload "freminal=1;fid=$fish_pid-$__freminal_fid_counter"
    end

    # ── fish_prompt event (A and B markers) ──────────────────────────────────
    # fish_prompt event handlers fire BEFORE the user's fish_prompt function
    # body executes.  vendor_conf.d guarantees we're registered before user
    # themes (Tide, Starship, etc.) get a chance to install their own handlers.
    function __freminal_fish_prompt --on-event fish_prompt
        __freminal_fid_next
        printf '\033]133;A;%s\007' $__freminal_fid_payload
        # We cannot inject B between the prompt text and the user's typed input
        # without overriding fish_prompt itself (which would conflict with user
        # themes).  Emitting B immediately after A still allows Freminal to
        # bracket the prompt region; user input still follows visually.  A and
        # B share the same fid (this command's lifecycle).
        printf '\033]133;B;%s\007' $__freminal_fid_payload
    end

    # ── fish_preexec event (C marker) ────────────────────────────────────────
    # C marks the start of command execution; shares the fid established at
    # the most recent fish_prompt.  No fid roll here.
    function __freminal_fish_preexec --on-event fish_preexec
        printf '\033]133;C;%s\007' $__freminal_fid_payload
    end

    # ── fish_postexec event (D marker + OSC 7 cwd) ───────────────────────────
    # D closes this command's block, reusing the same fid as A/B/C.  The next
    # fish_prompt will roll the counter for the next command's lifecycle.
    function __freminal_fish_postexec --on-event fish_postexec
        set -l __freminal_exit $status
        printf '\033]133;D;%s;%s\007' $__freminal_exit $__freminal_fid_payload
        set -l __freminal_hostname (hostname 2>/dev/null; or echo localhost)
        printf '\033]7;file://%s%s\007' $__freminal_hostname $PWD
    end

    # ── OSC 1338 HISTFILE report (Task 72.15) ────────────────────────────────
    # Report fish's history file path on the FIRST prompt cycle so freminal
    # can seed the Quick Command History Palette with the file fish actually
    # uses.
    #
    # vendor_conf.d files load BEFORE config.fish, so emitting at file-load
    # time would miss user overrides of $XDG_DATA_HOME or $fish_history.
    # Delaying until fish_prompt guarantees config.fish has run.  The handler
    # erases itself after firing so subsequent prompts pay no cost.
    #
    # Fish stores history at:
    #   ${XDG_DATA_HOME:-$HOME/.local/share}/fish/${fish_history:-fish}_history
    function __freminal_emit_histfile_once --on-event fish_prompt
        functions -e __freminal_emit_histfile_once
        set -l __freminal_session fish
        if set -q fish_history; and test -n "$fish_history"
            set __freminal_session $fish_history
        end
        set -l __freminal_base "$HOME/.local/share"
        if set -q XDG_DATA_HOME; and test -n "$XDG_DATA_HOME"
            set __freminal_base "$XDG_DATA_HOME"
        end
        printf '\033]1338;HISTFILE=%s/fish/%s_history\007' "$__freminal_base" "$__freminal_session"
    end
else
    functions -e __freminal_should_init
end
