# grit — the dashboard at the prompt.
#
#   eval "$(grit shell init bash)"
#
# Ctrl-G draws the table above the line you are typing; Ctrl-G again takes it
# back down. Pressing enter leaves it in the scrollback, where it belongs.
#
# bash has no hook that fires while you sit at the prompt, so unlike zsh this
# cannot appear on its own — the key is the whole interface.
#
#   GRIT_PREVIEW_KEY   the binding, in readline syntax; this replaces
#                      readline's `abort`. Default \C-g.
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

: "${GRIT_PREVIEW_KEY:=\C-g}"

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
case ";${PROMPT_COMMAND};" in
	*";__grit_preview_forget;"*) ;;
	*) PROMPT_COMMAND="__grit_preview_forget${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;;
esac

# Ctrl-G and not, say, Ctrl-O: ^W ^U ^V ^O ^S ^Q ^Z are tty line-discipline
# characters and never reach readline, so binding one of those succeeds and
# then quietly does nothing.
bind -x "\"${GRIT_PREVIEW_KEY}\": __grit_preview_toggle" 2>/dev/null
