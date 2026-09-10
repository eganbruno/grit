# grit — the dashboard at the prompt.
#
#   grit shell init fish | source
#
# ^G^P draws the table above the line you are typing; ^G^P again takes it back
# down. Pressing enter leaves it in the scrollback, where it belongs. ^G^G
# opens the repo picker.
#
# fish runs nothing while you sit at the prompt, so unlike zsh this cannot
# appear on its own — the key is the whole interface.
#
#   $GRIT_PREVIEW_KEY   the binding, in fish's escape syntax. Default \cg\cp.
#   $GRIT_PICKER_KEY    the repo picker's chord. Default \cg\cg; needs fzf.
#
# Both are chords under \cg and neither is bound to \cg alone: a bare prefix
# waits for the rest of a sequence that could still match a longer binding, so
# it would pause before firing every time. Not \cg\cd: on an empty line the
# \cd is end-of-input and closes the shell rather than firing.
#
# fish emits nothing before a bound function and does not repaint afterwards,
# so both ends are ours: erase first, print, then ask for the repaint.

status is-interactive; or exit 0

set -q GRIT_PREVIEW_KEY; or set -g GRIT_PREVIEW_KEY \cg\cp
set -q GRIT_PICKER_KEY; or set -g GRIT_PICKER_KEY \cg\cg

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

# The repo picker. fzf navigates; grit supplies the rows and the detail pane.
function __grit_picker
    if not command -q fzf
        printf '\ngrit: the repo picker needs fzf on your PATH\n'
        commandline -f repaint
        return 0
    end

    # Ours is on screen and fzf is about to paint over it.
    if test $__grit_preview_rows -gt 0
        __grit_preview_erase
        set -g __grit_preview_rows 0
    end

    set -l rows (command grit --color=always shell rows --cached 2>/dev/null | string collect)
    if test -z "$rows"
        commandline -f repaint
        return 0
    end

    set -l picked (printf '%s\n' $rows | command fzf \
        --ansi \
        --no-sort \
        --layout=reverse \
        --height="$GRIT_PICKER_HEIGHT" \
        --preview 'grit --color=always detail {1}' \
        --preview-window="$GRIT_PICKER_PREVIEW" \
        --header 'enter: run git here · ctrl-d: cd · ctrl-r: refresh' \
        --bind 'ctrl-r:reload(grit --color=always shell rows)' \
        --bind "ctrl-d:become(printf '%s\tcd' {1})" \
        --bind "enter:become(printf '%s\trun' {1})" | string collect)

    commandline -f repaint
    test -z "$picked"; and return 0

    set -l parts (string split -m1 \t -- $picked)
    set -l alias $parts[1]
    test -z "$alias"; and return 0

    if test (count $parts) -gt 1; and test "$parts[2]" = cd
        set -l dir (command grit shell path $alias 2>/dev/null)
        test -n "$dir"; and test -d "$dir"; and cd -- $dir
        commandline -f repaint
        return 0
    end

    commandline -r "grit $alias "
    commandline -f end-of-line
end

# Defaults for the picker's window, set here rather than inline so the fzf
# call above stays one shape whether or not they were exported.
set -q GRIT_PICKER_HEIGHT; or set -g GRIT_PICKER_HEIGHT 80%
set -q GRIT_PICKER_PREVIEW; or set -g GRIT_PICKER_PREVIEW right,55%,wrap

# `\cg`, never `ctrl-g`: fish 3.x accepts the new-style name with exit status 0
# and binds the literal six characters instead of the key.
if test -n "$GRIT_PREVIEW_KEY"
    bind $GRIT_PREVIEW_KEY __grit_preview_toggle
    bind -M insert $GRIT_PREVIEW_KEY __grit_preview_toggle 2>/dev/null
end

if test -n "$GRIT_PICKER_KEY"
    bind $GRIT_PICKER_KEY __grit_picker
    bind -M insert $GRIT_PICKER_KEY __grit_picker 2>/dev/null
end
