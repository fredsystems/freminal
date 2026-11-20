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

    # ── sudo keeps TERMINFO (Task 151.12; freminal's own wrapper, Task 151.C5) ─
    # sudo's environment reset drops TERMINFO, so programs run under sudo lose
    # freminal's terminfo entry.  When $FREMINAL_SHELL_FEATURES (a comma-separated
    # list) names the item `sudo`, TERMINFO is set and no `sudo` function or
    # alias exists yet, `sudo` becomes a function that runs the real sudo with
    # `--preserve-env=TERMINFO` first.  An edit invocation (`-e`, `--edit`,
    # including grouped or after other options) is passed through unchanged,
    # because sudo refuses `--preserve-env` with an edit.  This is freminal's
    # own implementation, written from sudo(8)'s option syntax.
    if contains -- sudo (string split -- , "$FREMINAL_SHELL_FEATURES"); and test -n "$TERMINFO"; and not functions -q sudo
        # Prints the full name of a long sudo option (given without the leading
        # dashes) the way sudo resolves it: an exact name wins, otherwise a prefix
        # of exactly one name.  Prints nothing for an unknown or ambiguous one.
        function __freminal_sudo_long_name
            test -n "$argv[1]"; or return 0
            set -l length (string length -- "$argv[1]")
            set -l matches
            for candidate in askpass auth-type background bell chdir chroot close-from command-timeout edit group help host list login login-class no-update non-interactive other-user preserve-env preserve-groups prompt remove-timestamp reset-timestamp role set-home shell stdin type user validate version
                if test "$candidate" = "$argv[1]"
                    echo $candidate
                    return 0
                end
                set -l head (string sub --length $length -- $candidate)
                if test "$head" = "$argv[1]"
                    set -a matches $candidate
                end
            end
            if test (count $matches) -eq 1
                echo $matches[1]
            end
        end

        # Succeeds when the sudo arguments request an edit before the command word.
        function __freminal_sudo_is_edit
            set -l takes_value no
            for word in $argv
                if test $takes_value = yes
                    set takes_value no
                    continue
                end
                if test $takes_value = host
                    # After a standalone `-h`: the next word is the host unless it is an
                    # option or a NAME=value assignment (a path-like word is a host).
                    set takes_value no
                    switch "$word"
                        case '-*'
                        case '/*' '=*'
                            continue
                        case '*=*'
                        case '*'
                            continue
                    end
                end
                switch "$word"
                    case --
                        return 1
                    case '--*'
                        set -l name (string replace -r -- '^--([^=]*).*' '$1' "$word")
                        set -l long (__freminal_sudo_long_name "$name")
                        switch "$long"
                            case edit
                                return 0
                            case auth-type chdir chroot close-from command-timeout group host login-class other-user prompt role type user
                                # Takes its value from the next word unless given as --name=value.
                                if not string match -q -- '*=*' "$word"
                                    set takes_value yes
                                end
                        end
                    case -
                        return 1
                    case '-*'
                        set -l rest (string sub --start 2 -- "$word")
                        while test -n "$rest"
                            set -l letter (string sub --length 1 -- "$rest")
                            set rest (string sub --start 2 -- "$rest")
                            switch $letter
                                case e
                                    return 0
                                case a C c D g p R r T t U u
                                    # The value is the rest of the word, or the next word.
                                    if test -z "$rest"
                                        set takes_value yes
                                    end
                                    break
                                case h
                                    # An optional value: the rest of this word; a standalone `-h`
                                    # also takes the next word as the host (see above).
                                    if test "$word" = -h
                                        set takes_value host
                                    end
                                    break
                            end
                        end
                    case '*=*'
                    case '*'
                        return 1
                end
            end
            return 1
        end

        function sudo -d "freminal: keep TERMINFO across sudo"
            if __freminal_sudo_is_edit $argv
                command sudo $argv
            else
                command sudo --preserve-env=TERMINFO $argv
            end
        end
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
