# grit — the dashboard at the prompt, before you press enter.
#
#   grit shell enable        installs the line below into your startup file
#   eval "$(grit shell init zsh)"
#
# Type `grit`, pause, and the table appears under the line you are editing.
# Type anything else and it goes. ^G^P draws it whenever you want it, and
# ^G^G opens the repo picker.
#
# Settings, set before the eval:
#
#   GRIT_PREVIEW_TRIGGERS  array of buffers that summon it      (grit)
#   GRIT_PREVIEW_DELAY     seconds of stillness first; may be    (0.5)
#                          fractional
#   GRIT_PREVIEW_KEY       key that draws it on demand        (^G^P)
#                          Empty binds nothing.
#   GRIT_PREVIEW_IDLE      0 for the key only, no timer         (1)
#   GRIT_PICKER_KEY        chord that opens the repo picker   (^G^G)
#                          Empty binds nothing. Needs fzf.
#
# Both are chords under `^G`, and neither is bound to `^G` alone. That is not
# ceremony. A line editor resolves an ambiguous prefix by waiting for the next
# key — 404ms in zsh, 504ms in readline, measured — and it charges that wait to
# the *shorter* binding, so anything left on a bare `^G` pauses before it fires,
# every time. The letters avoid fzf-git.sh's family (`^G^{f,b,t,r,h,s,l,e,w}`)
# as well, since that tool is common and claims `^G` as a prefix too. Setting
# GRIT_PREVIEW_KEY='^G' still works; it will just pause.
#
# The preview is not on `^G^D`, which is the obvious mnemonic for a dashboard
# and is unusable: on an empty command line that `^D` is read as end-of-input,
# so it never fires the widget and closes the shell instead. `^P` is what this
# feature is called in every other name here.
#
# Most of what follows is shaped by measured zsh 5.9 behaviour rather than by
# taste. The load-bearing parts:
#
#   * The timer is a file descriptor watched with `zle -F`, not `TMOUT` and no
#     longer `zsh/sched`. TMOUT is read only when the line read begins, so it
#     cannot be armed mid-line at all. `sched` can, and was what this used, but
#     its time specifier is whole seconds — `sched +0.5` is a parse error — and
#     half a second is the difference between a pause you choose and a pause you
#     wait out. So arming now opens a descriptor on a subshell that goes readable
#     after the delay, and ZLE wakes us when it does.
#
#     The cost is one short-lived subshell per arm, which is why the delay is
#     read in `zselect` centiseconds from a builtin rather than by exec'ing
#     `sleep`. Note this is per *trigger match*, not per prompt: a sleeper on
#     every prompt is the thing that keeps fish on its key binding alone.
#
#     Either way this never touches TMOUT or TRAPALRM, so it cannot quietly
#     disable someone's auto-logout, and nothing is being watched at all except
#     in the half second after you have typed the trigger.
#   * POSTDISPLAY is read-only outside a widget, and setting it draws nothing
#     without `zle -R`.
#   * The trigger is decided when the timer is armed and checked again inside
#     the widget. That second check is what makes a timer outliving its line
#     harmless, and it is why the handler calls a widget rather than drawing.
#   * A watch is removed with `zle -F <fd>` and no handler. `zle -F -<fd>` is a
#     parse error, not a negation — and a watch left on a closed descriptor
#     starves every other watch in ZLE's select set, which is somebody else's
#     prompt hanging on "loading" for the life of the shell.
#   * The clear in `line-pre-redraw` is unconditional and first. accept-line
#     redraws while the buffer is still the trigger, so a clear behind an `if`
#     leaves the table stranded above the command's own output.
#   * POSTDISPLAY is a single slot that zsh-autosuggestions also wants. The
#     table is *appended* to whatever is already there and removed by exact
#     suffix, so the two coexist instead of erasing each other.

if [[ -o interactive ]]; then

typeset -ga GRIT_PREVIEW_TRIGGERS
(( $#GRIT_PREVIEW_TRIGGERS )) || GRIT_PREVIEW_TRIGGERS=( grit )
: ${GRIT_PREVIEW_DELAY:=0.5}
: ${GRIT_PREVIEW_KEY=^G^P}
: ${GRIT_PREVIEW_IDLE:=1}
: ${GRIT_PICKER_KEY=^G^G}

typeset -ga _grit_preview_hl=()
typeset -g  _grit_preview_text=
typeset -gi _grit_preview_shown=0
typeset -gi _grit_preview_armed=0
typeset -gi _grit_preview_tries=0
typeset -gi _grit_preview_fd=0

# How long to keep retrying a line before giving up, in seconds. More than one
# attempt happens only when the last reading was too old to show and a fresh one
# is still being taken, so this is the patience for a slow set of repositories.
#
# Seconds rather than a count of attempts, because the two stop agreeing the
# moment the delay is not one second: five attempts at the old whole-second
# timer meant five seconds of patience, and at the current default it would mean
# two and a half — halving the delay would quietly halve how long grit waits for
# a slow refresh, which is not what changing a delay should do.
#
# A constant, but deliberately not `typeset -r`, and guarded on top of that.
#
# Read-only is the obvious way to write a constant and it makes the *second*
# sourcing of this script fatal: `typeset` over a read-only name is an error,
# the eval aborts on this line, and every widget, hook and binding below it is
# silently never installed — the shell looks like it loaded and does nothing.
#
# Dropping the `-r` is not enough on its own, because the name may already be
# read-only from a script this one did not write. grit 0.2.0 declared it
# `typeset -gir`, so on the one path that matters most — an rc file loads the
# installed grit, then you eval a newer one to try it — a plain `typeset -gi`
# here still lands on a read-only name and still aborts. Hence the guard: if
# something has already defined it, leave it alone.
(( ${+_grit_preview_patience} )) || typeset -gi _grit_preview_patience=5

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

	# One column short of the terminal, deliberately. A line drawn right up
	# to the last column trips the auto-margin: the cursor wraps past it and
	# the table takes one more screen row than it has newlines, so zsh's own
	# count comes up short and the bottom row of the previous draw is never
	# erased. `grit.bash` and `grit.fish` reserve the column for this too.
	local -i width=$(( ${COLUMNS:-80} - 1 ))
	(( width >= 20 )) || width=20

	local -a out
	out=( "${(@f)$(COLUMNS=$width command grit shell preview --max-rows $rows --max-age 60 2>/dev/null)}" )
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
		# Quoted, and that is the whole of it: `a=( ${x} )` with x empty makes
		# an array of *no* elements, not one empty element. Unquoted, the key
		# did nothing at all on an empty command line — the trigger list it
		# built to match the buffer against had nothing in it to match.
		GRIT_PREVIEW_TRIGGERS=( "${BUFFER%"${BUFFER##*[![:space:]]}"}" )
		_grit_preview_show
		GRIT_PREVIEW_TRIGGERS=( $keep )
	fi
	return 0
}
zle -N _grit_preview_toggle

# Open a descriptor that goes readable every `GRIT_PREVIEW_DELAY`, and have ZLE
# wake us on it.
#
# One subshell that keeps ticking, rather than a fresh one per tick. A handler
# re-arming itself does work — measured, once the removal below was correct —
# so this is a choice and not a necessity: one fork per line of typing rather
# than one per half second, and stopping is then our job rather than the
# sleeper's.
_grit_preview_arm() {
	(( _grit_preview_armed )) && return 0

	# Whatever the flag said, a descriptor still open here is one whose number we
	# are about to overwrite — and losing the number orphans its ticker for the
	# life of the shell, writing into a pipe nobody is left to close.
	_grit_preview_release

	# Centiseconds, because that is what `zselect -t` speaks. Assigning a float
	# expression to an integer truncates, which is the rounding we want: a delay
	# too small to express is a delay of nothing, not an error.
	local -i centis=$(( GRIT_PREVIEW_DELAY * 100 ))
	(( centis > 0 )) || centis=1

	exec {_grit_preview_fd}< <(while :; do zselect -t $centis 2>/dev/null; print -n x || break; done)
	(( _grit_preview_fd )) || return 0

	zle -F $_grit_preview_fd _grit_preview_fire
	_grit_preview_armed=1
	return 0
}

# Stop watching and close our end, which is what the ticker is waiting to be
# told: its next write into a pipe with no reader costs it one SIGPIPE.
#
# Deliberately not guarded on `_grit_preview_armed`. The flag and the descriptor
# can disagree — `_grit_preview_fire` runs with one released and the other not,
# and TRAPINT can arrive inside that window — and a disarm that believed the
# flag would return with a descriptor still open. The descriptor is the truth
# here, so the flag is set from the same place that closes it.
_grit_preview_disarm() {
	_grit_preview_release
	return 0
}

_grit_preview_release() {
	_grit_preview_armed=0
	(( _grit_preview_fd )) || return 0

	# `zle -F <fd>` with the handler left off is how a watch is removed.
	# `zle -F -<fd>` is not the negation of anything: it is a parse error
	# ("Bad file descriptor number"), and with stderr thrown away it looks
	# exactly like success. The descriptor then gets closed underneath a watch
	# that is still installed, and a dead descriptor in ZLE's select set starves
	# *every* watch in it — including the one powerlevel10k's gitstatus uses for
	# its daemon, which is left saying "loading" for the life of the shell.
	zle -F $_grit_preview_fd 2>/dev/null

	# The braces are the whole point. A redirection written on a bare `exec` is
	# not scoped to it — `exec {fd}<&- 2>/dev/null` closes the descriptor *and*
	# points the interactive shell's own stderr at /dev/null for the life of the
	# shell, so every error message anything prints from then on is thrown away.
	# It is silent, it survives the prompt, and it looks like the command did
	# nothing. Redirecting the group instead leaves fd 2 alone.
	{ exec {_grit_preview_fd}<&- } 2>/dev/null
	_grit_preview_fd=0
	return 0
}

# Called by `zle -F` with the descriptor as its first argument.
_grit_preview_fire() {
	local fd=$1

	# Consume the tick first. An unread descriptor stays readable, and ZLE would
	# call this straight back with no pause at all.
	local tick
	read -r -k1 -u $fd tick 2>/dev/null

	(( _grit_preview_tries++ ))

	# Behind the table, top up the reading it is drawn from. grit throttles
	# this itself, so a warm cache costs one process exit and no git at all.
	#
	# It closes the ticker's descriptor on the way in. A child inheriting the
	# read end is a reader, and while it lives the ticker's writes succeed — so a
	# refresh outliving its disarm keeps a sleeper alive behind it.
	( { exec {_grit_preview_fd}<&- } 2>/dev/null
	  command grit shell refresh --max-age 30 & ) >/dev/null 2>&1

	zle _grit_preview_show 2>/dev/null

	# Nothing drawn means the last reading was too old to put in front of
	# someone, and the refresh just started is what will fix that. Leaving the
	# ticker running is how we come back for it — for as long as
	# `_grit_preview_patience`, then stop, so a repository that cannot be read
	# at all does not turn this into a permanent spin.
	local -i max=$(( _grit_preview_patience / GRIT_PREVIEW_DELAY ))
	(( max > 0 )) || max=1
	if (( _grit_preview_shown || _grit_preview_tries >= max )); then
		_grit_preview_release
	fi
	return 0
}

# Open the repo picker: every repo on the left, `grit detail` on the right.
#
# fzf does the navigating. Not a shortcut taken for want of a terminal library —
# aligned columns and colour for meaning is what grit's table already is, and a
# picker over rows grit prints is the whole feature. grit supplies the two
# halves: `shell rows` for the list, `detail` for the pane.
#
# The widget runs in this shell rather than in a child, which is what lets the
# cd binding change this shell's directory. fzf opens the terminal itself, so
# the command substitution carries back only the answer.
_grit_picker() {
    emulate -L zsh

    if (( ! $+commands[fzf] )); then
        zle -M "grit: the repo picker needs fzf on your PATH"
        return 0
    fi

    # The preview lives in POSTDISPLAY and fzf is about to paint over those
    # same rows; taking it down first is what stops it being stranded there.
    _grit_preview_hide
    _grit_preview_disarm

    local picked
    picked=$(_grit_picker_run)

    # Unconditional and first: fzf has repainted the screen whether or not
    # anything was chosen.
    zle reset-prompt
    [[ -n $picked ]] || return 0

    local alias=${picked%%$'\t'*}
    local action=${picked#*$'\t'}
    [[ -n $alias ]] || return 0

    if [[ $action == cd ]]; then
        local dir
        dir=$(command grit shell path $alias 2>/dev/null)
        if [[ -n $dir && -d $dir ]]; then
            builtin cd -- $dir
            zle reset-prompt
        fi
        return 0
    fi

    # Left on the line rather than run. Which git command you wanted is the
    # part grit cannot guess, and a picker that ran something on Enter would be
    # one you could not use to start a commit.
    BUFFER="grit $alias "
    CURSOR=$#BUFFER
    return 0
}
zle -N _grit_picker

# The fzf invocation, kept apart so the widget above reads as what it does.
#
# `--cached` is why it opens instantly: the reading the inline preview draws
# from, with a live one taken only if there is not one yet. ctrl-r reloads from
# a fresh reading, which is the one path here allowed to be slow.
#
# `become` rather than plain acceptance, because two of the bindings have to
# say which of them fired: fzf replaces itself with the printf and its output
# is what the substitution above reads.
_grit_picker_run() {
    emulate -L zsh

    local rows
    rows=$(command grit --color=always shell rows --cached 2>/dev/null)
    [[ -n $rows ]] || return 0

    print -r -- $rows | command fzf \
        --ansi \
        --no-sort \
        --layout=reverse \
        --height=${GRIT_PICKER_HEIGHT:-80%} \
        --preview 'grit --color=always detail {1}' \
        --preview-window=${GRIT_PICKER_PREVIEW:-right,55%,wrap} \
        --header 'enter: run git here · ctrl-d: cd · ctrl-r: refresh' \
        --bind 'ctrl-r:reload(grit --color=always shell rows)' \
        --bind "ctrl-d:become(printf '%s\tcd' {1})" \
        --bind "enter:become(printf '%s\trun' {1})"
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
# Both of these `zle -N`s look redundant — the functions are only ever reached
# through `add-zle-hook-widget` below — and removing either silently costs the
# idle preview entirely. `add-zle-hook-widget` takes a *widget*, so an argument
# that is merely a function is rejected and not added to the hook's list, with
# nothing said about it. `add-zle-hook-widget -L line-pre-redraw` is what shows
# you: an unregistered name is simply absent from the zstyle it prints.
zle -N _grit_preview_on_finish

# `zsh/zselect` is what the timer's sleeper counts with, and it ships with
# zsh — but the key binding should still work on a build without it, so a
# failure here costs the idle preview and nothing else. Checking the builtin
# rather than `zmodload`'s exit status is deliberate: a load can report
# success and still leave nothing callable, and the subshell would then race
# straight past its delay and draw instantly.
if ! { zmodload -F zsh/zselect b:zselect 2>/dev/null && (( ${+builtins[zselect]} )) }; then
	GRIT_PREVIEW_IDLE=0
fi

add-zle-hook-widget line-pre-redraw _grit_preview_on_redraw
# Enter within the delay would otherwise leave a timer to fire at the next
# prompt. Harmless — the widget rechecks the buffer — but it is a wakeup.
add-zle-hook-widget line-finish _grit_preview_on_finish

# Bound exactly as asked, with no rearranging. An earlier version moved a
# binding out of the way when it was a prefix of the other, which is one more
# thing to explain and was wrong the moment a third chord existed: doubling
# `^G` lands on `^G^G`, which is the picker.
if [[ -n $GRIT_PREVIEW_KEY ]]; then
    bindkey $GRIT_PREVIEW_KEY _grit_preview_toggle
fi

if [[ -n $GRIT_PICKER_KEY ]]; then
    bindkey $GRIT_PICKER_KEY _grit_picker
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
