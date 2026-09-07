# grit — the dashboard at the prompt.
#
#   grit shell init fish | source
#
# Ctrl-G draws the table above the line you are typing; Ctrl-G again takes it
# back down. Pressing enter leaves it in the scrollback, where it belongs.
#
# fish runs nothing while you sit at the prompt, so unlike zsh this cannot
# appear on its own — the key is the whole interface.
#
#   $GRIT_PREVIEW_KEY   the binding, in fish's escape syntax. Default \cg.
#
# fish emits nothing before a bound function and does not repaint afterwards,
# so both ends are ours: erase first, print, then ask for the repaint.

status is-interactive; or exit 0

set -q GRIT_PREVIEW_KEY; or set -g GRIT_PREVIEW_KEY \cg

# Rows of ours currently on screen; 0 when nothing is showing.
set -g __grit_preview_rows 0

function __grit_preview_text
    set -lx COLUMNS (math max 20, "$COLUMNS" - 1)

    # You pressed a key and are about to read the answer, so it had better not
    # be an hour old. Costs nothing when the cache is already warm, and this is
    # the one press that is allowed to be slow when it is not.
    command grit shell refresh --max-age 60 >/dev/null 2>&1

    # `string collect` rather than "$(...)": the latter needs fish 3.4.
    set -l out (command grit status --cached --color=always 2>/dev/null | string collect)
    if test -z "$out"
        set out (command grit status --color=always 2>/dev/null | string collect)
    end

    printf '%s' $out
end

function __grit_preview_erase
    if test $__grit_preview_rows -gt 0
        printf '\033[%dA' $__grit_preview_rows
    end
    printf '\r\033[J'
end

function __grit_preview_toggle
    if test $__grit_preview_rows -gt 0
        __grit_preview_erase
        set -g __grit_preview_rows 0
        commandline -f repaint
        return 0
    end

    set -l out (__grit_preview_text | string collect)
    if test -z "$out"
        commandline -f repaint
        return 0
    end

    __grit_preview_erase
    printf '%s\n' $out
    set -g __grit_preview_rows (count (string split \n -- $out))
    commandline -f repaint
end

# A command ran, so what we drew is real scrollback now rather than something
# of ours to erase. fish has no PROMPT_COMMAND; this is the equivalent.
function __grit_preview_forget --on-event fish_preexec
    set -g __grit_preview_rows 0
end

# `\cg`, never `ctrl-g`: fish 3.x accepts the new-style name with exit status 0
# and binds the literal six characters instead of the key.
bind $GRIT_PREVIEW_KEY __grit_preview_toggle
bind -M insert $GRIT_PREVIEW_KEY __grit_preview_toggle 2>/dev/null
