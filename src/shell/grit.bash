# grit — the dashboard at the prompt.
#
#   eval "$(grit shell init bash)"
#
# ^G^P draws the table above the line you are typing; ^G^P again takes it back
# down. Pressing enter leaves it in the scrollback, where it belongs. ^G^G
# opens the repo picker.
#
# bash has no hook that fires while you sit at the prompt, so unlike zsh this
# cannot appear on its own — the key is the whole interface.
#
#   GRIT_PREVIEW_KEY   the binding, in readline syntax. Default \C-g\C-p.
#   GRIT_PICKER_KEY    the repo picker's chord. Default \C-g\C-g; needs fzf.
#
# Both are chords under \C-g and neither is bound to \C-g alone: readline
# resolves an ambiguous prefix by waiting keyseq-timeout for the next key —
# 504ms, measured on bash 5.3 — and charges it to the shorter binding, every
# press. The letters also stay clear of fzf-git.sh's family.
#
# Not \C-g\C-d: on an empty line the \C-d is end-of-input and logs you out
# rather than firing. ^W ^U ^V ^O ^S ^Q ^Z never reach readline at all.
#
# readline redraws the prompt and what you have typed after the bound function
# returns, so the function only has to print. What it must *not* do is print a
# leading newline, and what it must get right is how far back up to go when
# erasing — bash 3.x abandons the prompt row above us where bash 4.0+ erases it
# and leaves us standing on it. That one row is the difference between erasing
# cleanly and eating a line of scrollback every time.

case $- in
	*i*) ;;
	*) return 0 ;;
esac

: "${GRIT_PREVIEW_KEY:=\C-g\C-p}"
: "${GRIT_PICKER_KEY=\C-g\C-g}"

# Rows of ours currently on screen; 0 when nothing is showing.
__grit_preview_rows=0

__grit_preview_text() {
	local width=$(( ${COLUMNS:-80} - 1 ))

	# You pressed a key and are about to read the answer, so it had better not
	# be an hour old. Costs nothing when the cache is already warm, and this is
	# the one press that is allowed to be slow when it is not.
	command grit shell refresh --max-age 60 >/dev/null 2>&1

	local out
	out=$(COLUMNS=$width command grit status --cached --color=always 2>/dev/null)
	if [ -z "$out" ]; then
		out=$(COLUMNS=$width command grit status --color=always 2>/dev/null)
	fi

	printf '%s' "$out"
}

__grit_preview_erase() {
	local up=$__grit_preview_rows

	# bash 3.x emitted \r\n before calling us, so the prompt is one row up.
	# bash 4.0+ emitted \r\e[K, which put us on it.
	if [ "${BASH_VERSINFO[0]}" -lt 4 ]; then
		up=$(( up + 1 ))
	fi

	[ "$up" -gt 0 ] && printf '\033[%dA' "$up"
	printf '\r\033[J'
}

__grit_preview_toggle() {
	if [ "$__grit_preview_rows" -gt 0 ]; then
		__grit_preview_erase
		__grit_preview_rows=0
		return 0
	fi

	local out
	out=$(__grit_preview_text)
	[ -z "$out" ] && return 0

	__grit_preview_erase
	printf '%s\n' "$out"
	__grit_preview_rows=$(printf '%s\n' "$out" | wc -l | tr -d ' ')
}

# A command ran, so what we drew is real scrollback now rather than something
# of ours to erase.
__grit_preview_forget() { __grit_preview_rows=0; }

# The repo picker. fzf navigates; grit supplies the rows and the detail pane.
#
# Unlike the preview above, this does not draw anything itself — fzf owns the
# screen while it runs and repaints on the way out — so there is no row count
# to keep. What it leaves behind is a command on the line, which is why the
# readline variables are poked directly rather than printed.
__grit_picker() {
    if ! command -v fzf >/dev/null 2>&1; then
        printf '\ngrit: the repo picker needs fzf on your PATH\n'
        return 0
    fi

    # Ours is on screen and fzf is about to paint over it.
    if [ "$__grit_preview_rows" -gt 0 ]; then
        __grit_preview_erase
        __grit_preview_rows=0
    fi

    local rows picked alias action
    rows=$(command grit --color=always shell rows --cached 2>/dev/null)
    [ -z "$rows" ] && return 0

    picked=$(printf '%s\n' "$rows" | command fzf \
        --ansi \
        --no-sort \
        --layout=reverse \
        --height="${GRIT_PICKER_HEIGHT:-80%}" \
        --preview 'grit --color=always detail {1}' \
        --preview-window="${GRIT_PICKER_PREVIEW:-right,55%,wrap}" \
        --header 'enter: run git here · ctrl-d: cd · ctrl-r: refresh' \
        --bind 'ctrl-r:reload(grit --color=always shell rows)' \
        --bind "ctrl-d:become(printf '%s\tcd' {1})" \
        --bind "enter:become(printf '%s\trun' {1})")

    [ -z "$picked" ] && return 0
    alias=${picked%%$'\t'*}
    action=${picked#*$'\t'}
    [ -z "$alias" ] && return 0

    if [ "$action" = cd ]; then
        local dir
        dir=$(command grit shell path "$alias" 2>/dev/null)
        [ -n "$dir" ] && [ -d "$dir" ] && cd -- "$dir"
        return 0
    fi

    # READLINE_LINE/POINT are how a `bind -x` function edits the line it was
    # called from; printing would put the text in the scrollback instead.
    READLINE_LINE="grit $alias "
    READLINE_POINT=${#READLINE_LINE}
}

# bash 5.1 allowed `PROMPT_COMMAND` to be an array, and prompt frameworks use
# it that way. Assigning a string to one does not drop the other elements —
# bash writes element 0 — but it does rewrite a line somebody else owns into
# `__grit_preview_forget;their command`, and the string check then only ever
# inspects element 0, so a marker sitting in element 1 reads as absent and we
# prepend a second time. Adding our own element leaves theirs alone, which is
# the posture the rest of this integration takes.
#
# `${VAR@a}` needs bash 4.4, so it sits behind the version test rather than
# beside it: `&&` is lazy, and on bash 3.2 the expansion is a runtime "bad
# substitution" that is simply never reached.
if [ "${BASH_VERSINFO[0]}" -ge 5 ] && [[ ${PROMPT_COMMAND@a} == *a* ]]; then
	__grit_pc_seen=0
	for __grit_pc in "${PROMPT_COMMAND[@]}"; do
		case $__grit_pc in
			*__grit_preview_forget*) __grit_pc_seen=1 ;;
		esac
	done
	if [ "$__grit_pc_seen" -eq 0 ]; then
		PROMPT_COMMAND=(__grit_preview_forget "${PROMPT_COMMAND[@]}")
	fi
	unset __grit_pc __grit_pc_seen
else
	case ";${PROMPT_COMMAND};" in
		*";__grit_preview_forget;"*) ;;
		*) PROMPT_COMMAND="__grit_preview_forget${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;;
	esac
fi

# ^W ^U ^V ^O ^S ^Q ^Z are tty line-discipline characters and never reach
# readline, so binding one of those succeeds and then quietly does nothing.
# ^D is worse than useless as the tail of a chord: on an empty line readline
# reads it as end-of-input, so it does not fire and the shell exits.
[ -n "$GRIT_PREVIEW_KEY" ] &&
    bind -x "\"${GRIT_PREVIEW_KEY}\": __grit_preview_toggle" 2>/dev/null

[ -n "$GRIT_PICKER_KEY" ] &&
    bind -x "\"${GRIT_PICKER_KEY}\": __grit_picker" 2>/dev/null
