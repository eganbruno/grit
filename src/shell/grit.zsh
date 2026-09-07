# grit — the dashboard at the prompt, before you press enter.
#
#   eval "$(grit shell init zsh)"
#
# Type `grit`, pause, and the table appears under the line you are editing.
# Type anything else and it goes. Ctrl-G draws it whenever you want it.
#
# Settings, set before the eval:
#
#   GRIT_PREVIEW_TRIGGERS  array of buffers that summon it      (grit)
#   GRIT_PREVIEW_DELAY     whole seconds of stillness first     (1)
#   GRIT_PREVIEW_KEY       key that draws it on demand; this    (^G)
#                          replaces zsh's `send-break`. Empty
#                          binds nothing.
#   GRIT_PREVIEW_IDLE      0 for the key only, no timer         (1)
#
# Most of what follows is shaped by measured zsh 5.9 behaviour rather than by
# taste. The load-bearing parts:
#
#   * The timer is `zsh/sched`, not `TMOUT`. ZLE consults the scheduled-function
#     list every time it recomputes its input timeout, so a sched entry can be
#     armed and cancelled part-way through a line — TMOUT is read only when the
#     line read begins and cannot be armed mid-line. It also means this never
#     touches TMOUT or TRAPALRM, so it cannot quietly disable someone's
#     auto-logout, and the shell is running no timer at all except in the second
#     after you have typed the trigger.
#   * POSTDISPLAY is read-only outside a widget, and setting it draws nothing
#     without `zle -R`.
#   * $BUFFER is not visible inside a scheduled function, so the trigger is
#     decided when the timer is armed and checked again inside the widget. That
#     second check is what makes a timer that outlives its line harmless.
#   * The clear in `line-pre-redraw` is unconditional and first. accept-line
#     redraws while the buffer is still the trigger, so a clear behind an `if`
#     leaves the table stranded above the command's own output.
#   * POSTDISPLAY is a single slot that zsh-autosuggestions also wants. The
#     table is *appended* to whatever is already there and removed by exact
#     suffix, so the two coexist instead of erasing each other.

if [[ -o interactive ]]; then

typeset -ga GRIT_PREVIEW_TRIGGERS
(( $#GRIT_PREVIEW_TRIGGERS )) || GRIT_PREVIEW_TRIGGERS=( grit )
: ${GRIT_PREVIEW_DELAY:=1}
: ${GRIT_PREVIEW_KEY=^G}
: ${GRIT_PREVIEW_IDLE:=1}

typeset -ga _grit_preview_hl=()
typeset -g  _grit_preview_text=
typeset -gi _grit_preview_shown=0
typeset -gi _grit_preview_armed=0
typeset -gi _grit_preview_tries=0

# How many one-second attempts to make before giving up on a line. Only more
# than one when the last reading was too old to show and a fresh one is still
# being taken, so this is the patience for a slow set of repositories.
typeset -gir _grit_preview_max_tries=5

autoload -Uz add-zle-hook-widget

_grit_preview_triggered() {
	local buf=${BUFFER%"${BUFFER##*[![:space:]]}"}
	(( ${GRIT_PREVIEW_TRIGGERS[(Ie)$buf]} ))
}

# Draw the table into POSTDISPLAY and its colours into region_highlight.
#
# `grit shell preview` answers with a count, that many `start end style` lines,
# and then the table. Those offsets are character counts from the table's first
# character; region_highlight indexes BUFFER+POSTDISPLAY, so the base is
# everything already in front of it.
_grit_preview_show() {
	emulate -L zsh

	# The timer may have outlived the line that armed it.
	_grit_preview_triggered || return 0
	(( _grit_preview_shown )) && return 0

	local -i rows=$(( ${LINES:-24} - 6 ))
	(( rows > 0 )) || rows=1

	local -a out
	out=( "${(@f)$(COLUMNS=${COLUMNS:-80} command grit shell preview --max-rows $rows --max-age 60 2>/dev/null)}" )
	(( $#out > 1 )) || return 0

	local -i n=${out[1]}
	local text=${(F)out[$(( n + 2 )),-1]}
	[[ -n $text ]] || return 0

	# Appended, not assigned: something else may already be showing a
	# suggestion here, and this is drawn underneath it rather than over it.
	local -i base=$(( ${#BUFFER} + ${#POSTDISPLAY} + 1 ))
	_grit_preview_text=$'\n'$text
	POSTDISPLAY=$POSTDISPLAY$_grit_preview_text
	_grit_preview_shown=1

	local -i i
	local -a range
	local spec
	for (( i = 2; i <= n + 1; i++ )); do
		range=( ${=out[i]} )
		(( $#range == 3 )) || continue
		spec="$(( base + range[1] )) $(( base + range[2] )) ${range[3]}"
		_grit_preview_hl+=( $spec )
		region_highlight+=( $spec )
	done

	zle -R
	return 0
}
zle -N _grit_preview_show

# Take it back down, touching nothing that is not ours: the table is removed by
# exact suffix and only our own highlight ranges are dropped.
_grit_preview_hide() {
	emulate -L zsh
	(( _grit_preview_shown )) || return 0
	_grit_preview_shown=0

	POSTDISPLAY=${POSTDISPLAY%"$_grit_preview_text"}
	_grit_preview_text=

	local spec
	for spec in $_grit_preview_hl; do
		region_highlight=( ${region_highlight:#$spec} )
	done
	_grit_preview_hl=()
	return 0
}
zle -N _grit_preview_hide

_grit_preview_toggle() {
	if (( _grit_preview_shown )); then
		_grit_preview_hide
		zle -R
	else
		# By hand, so the trigger does not have to match.
		local -a keep=( $GRIT_PREVIEW_TRIGGERS )
		GRIT_PREVIEW_TRIGGERS=( ${BUFFER%"${BUFFER##*[![:space:]]}"} )
		_grit_preview_show
		GRIT_PREVIEW_TRIGGERS=( $keep )
	fi
	return 0
}
zle -N _grit_preview_toggle

_grit_preview_arm() {
	(( _grit_preview_armed )) && return 0
	sched +$GRIT_PREVIEW_DELAY _grit_preview_fire
	_grit_preview_armed=1
	return 0
}

# Cancel our pending timer and nobody else's. Deleting from the highest index
# down keeps the lower ones where `sched` reported them.
_grit_preview_disarm() {
	(( _grit_preview_armed )) || return 0
	_grit_preview_armed=0

	local -a pending=( ${(f)"$(sched)"} )
	local -i i
	for (( i = $#pending; i >= 1; i-- )); do
		[[ $pending[i] == *_grit_preview_fire* ]] && sched -$i
	done
	return 0
}

_grit_preview_fire() {
	_grit_preview_armed=0
	(( _grit_preview_tries++ ))

	# Behind the table, top up the reading it is drawn from. grit throttles
	# this itself, so a warm cache costs one process exit and no git at all.
	(command grit shell refresh --max-age 30 &) >/dev/null 2>&1

	zle _grit_preview_show 2>/dev/null

	# Nothing drawn means the last reading was too old to put in front of
	# someone, and the refresh just started is what will fix that. Come back for
	# it — a few times, then stop, so a repository that cannot be read at all
	# does not turn this into a once-a-second spin.
	if (( ! _grit_preview_shown && _grit_preview_tries < _grit_preview_max_tries )); then
		_grit_preview_arm
	fi
	return 0
}

_grit_preview_on_redraw() {
	emulate -L zsh
	_grit_preview_hide

	if (( GRIT_PREVIEW_IDLE )) && _grit_preview_triggered; then
		(( _grit_preview_armed )) || _grit_preview_tries=0
		_grit_preview_arm
	else
		_grit_preview_disarm
	fi
	return 0
}
zle -N _grit_preview_on_redraw

_grit_preview_on_finish() {
	_grit_preview_disarm
	return 0
}
zle -N _grit_preview_on_finish

# `zsh/sched` ships with zsh, but the key binding should still work if some
# build of it does not.
if ! zmodload -F zsh/sched b:sched 2>/dev/null; then
	GRIT_PREVIEW_IDLE=0
fi

add-zle-hook-widget line-pre-redraw _grit_preview_on_redraw
# Enter within the delay would otherwise leave a timer to fire at the next
# prompt. Harmless — the widget rechecks the buffer — but it is a wakeup.
add-zle-hook-widget line-finish _grit_preview_on_finish

if [[ -n $GRIT_PREVIEW_KEY ]]; then
	bindkey $GRIT_PREVIEW_KEY _grit_preview_toggle
fi

# Ctrl-C reaches the shell as a signal, not as a key — ZLE runs no hook for it,
# and the table would be left stranded above the next prompt. Both halves here
# are load-bearing: going through the widget keeps zsh's own row bookkeeping in
# step, and returning 128+signal is what lets the interrupt through. Returning 0
# swallows it and the line is never aborted.
if ! (( ${+functions[TRAPINT]} )); then
	TRAPINT() {
		_grit_preview_disarm
		zle _grit_preview_hide 2>/dev/null
		zle -R 2>/dev/null
		return $(( 128 + $1 ))
	}
fi

fi
