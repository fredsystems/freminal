#!/run/current-system/sw/bin/bash
# Workload definitions shared by the terminal parity runner.

set -euo pipefail

readonly PARITY_SYSTEM_BASH="/run/current-system/sw/bin/bash"
declare -gx PARITY_NEW_WINDOW_ADDRESS=''
declare -gx PARITY_NEW_WINDOW_PID=''

# OSC 133 byte sequences for the sustained-output-marked workload. Freminal
# only records a prompt row and command block for markers tagged
# `freminal=1;fid=<id>` (freminal-common `parse_ftcs_params`); WezTerm and
# Ghostty ignore the extra key=value options. These printf formats are the
# single source of truth: a unit test in freminal-terminal-emulator reads this
# file and replays exactly these bytes, so changing them here without updating
# that test fails `cargo test`. `%d` is the per-burst command id.
readonly PARITY_MARKED_PROMPT_FORMAT='\033]133;A;freminal=1;fid=%d\007Parity prompt>\033]133;B;freminal=1;fid=%d\007\033]133;C;freminal=1;fid=%d\007'
readonly PARITY_MARKED_FINISH_FORMAT='\033]133;D;0;freminal=1;fid=%d\007'

# Resolve btop only when the btop workload actually runs, so sourcing this file
# never fails on a host without it.
parity_btop_bin() {
	local bin
	bin=$(command -v btop) || {
		printf 'btop workload requires btop on PATH\n' >&2
		return 1
	}
	printf '%s\n' "${bin}"
}

parity_require_system_bash() {
	if [[ ! -x "${PARITY_SYSTEM_BASH}" ]]; then
		printf 'The parity runner requires %s (NixOS bash-interactive).\n' "${PARITY_SYSTEM_BASH}" >&2
		return 1
	fi
}

parity_replace_token() {
	local file=$1 token=$2 value=$3
	python3 - "${file}" "${token}" "${value}" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
token = f"@{sys.argv[2]}@"
path.write_text(path.read_text(encoding="utf-8").replace(token, sys.argv[3]), encoding="utf-8")
PY
}

parity_render_fixtures() {
	local source_dir=$1 run_dir=$2 _terminal=$3 cursor=$4 marker=$5
	local blink_bool='' cursor_style=''

	case "${cursor}" in
	blink)
		blink_bool=true
		cursor_style=BlinkingBlock
		;;
	steady)
		blink_bool=false
		cursor_style=SteadyBlock
		;;
	*)
		printf 'unknown cursor mode: %s\n' "${cursor}" >&2
		return 2
		;;
	esac

	install -m 0600 "${source_dir}/shell.rc" "${run_dir}/shell.rc"
	install -m 0600 "${source_dir}/freminal.toml" "${run_dir}/freminal.toml"
	install -m 0600 "${source_dir}/wezterm.lua" "${run_dir}/wezterm.lua"
	install -m 0600 "${source_dir}/ghostty.conf" "${run_dir}/ghostty.conf"
	install -m 0600 "${source_dir}/btop.conf" "${run_dir}/btop.conf"

	parity_replace_token "${run_dir}/shell.rc" PARITY_MARKER "${marker}"
	parity_replace_token "${run_dir}/freminal.toml" CURSOR_BLINK "${blink_bool}"
	parity_replace_token "${run_dir}/wezterm.lua" CURSOR_BLINK_STYLE "${cursor_style}"
	parity_replace_token "${run_dir}/wezterm.lua" SHELL_RC "${run_dir}/shell.rc"
	parity_replace_token "${run_dir}/ghostty.conf" CURSOR_STYLE_BLINK "${blink_bool}"
	parity_replace_token "${run_dir}/ghostty.conf" SHELL_RC "${run_dir}/shell.rc"

	unset PARITY_FORCE_TERMINAL_ROWS PARITY_FORCE_TERMINAL_COLS
}

parity_wait_focused() {
	local address=$1 active
	for _ in {1..50}; do
		active=$(hyprctl activewindow -j | jq -r '.address')
		[[ ${active} == "${address}" ]] && return 0
		sleep 0.1
	done
	printf 'new window %s did not become focused; refusing synthetic input\n' "${address}" >&2
	return 1
}

parity_pty_path() {
	local shell_pid=$1 target
	target=$(readlink "/proc/${shell_pid}/fd/0" 2>/dev/null || true)
	[[ ${target} == /dev/pts/* ]] && printf '%s\n' "${target}" && return 0
	printf 'shell PID %s has no PTY on fd 0\n' "${shell_pid}" >&2
	return 1
}

parity_wait_ready() {
	local ready_file=$1 pid=$2 timeout_seconds=${3:-15}
	local end=$((SECONDS + timeout_seconds))
	while [[ ! -e "${ready_file}" ]]; do
		if ! kill -0 "${pid}" 2>/dev/null; then
			printf 'terminal process %s exited before shell readiness\n' "${pid}" >&2
			return 1
		fi
		if ((SECONDS >= end)); then
			printf 'timed out waiting for shell readiness: %s\n' "${ready_file}" >&2
			return 1
		fi
		sleep 0.1
	done
}

parity_wait_new_window() {
	local baseline_file=$1 expected_class=$2 expected_config=$3 timeout_seconds=${4:-15}
	local address pid class
	local end=$((SECONDS + timeout_seconds))
	while ((SECONDS < end)); do
		while IFS=$'\t' read -r address pid class; do
			if [[ ${class} == "${expected_class}" ]] &&
				! grep -Fxq -- "${address}" "${baseline_file}" &&
				tr '\0' '\n' <"/proc/${pid}/environ" 2>/dev/null |
				grep -Fxq -- "XDG_CONFIG_HOME=${expected_config}"; then
				PARITY_NEW_WINDOW_ADDRESS=${address}
				PARITY_NEW_WINDOW_PID=${pid}
				return 0
			fi
		done < <(hyprctl clients -j | jq -r '.[] | [.address, (.pid | tostring), .class] | @tsv')
		sleep 0.1
	done
	printf 'timed out waiting for a newly mapped Hyprland client\n' >&2
	return 1
}

# Synthetic input must only ever reach the window this run spawned. Called at
# the top of every input-loop iteration: if focus moved (the operator clicked
# away, an abort left the loop running, a notification stole focus) the loop
# stops rather than typing into the user's desktop.
parity_focus_guard() {
	local active
	[[ -n ${PARITY_NEW_WINDOW_ADDRESS} ]] || {
		printf 'no spawned window address recorded; refusing synthetic input\n' >&2
		return 1
	}
	active=$(hyprctl activewindow -j | jq -r '.address') || return 1
	[[ ${active} == "${PARITY_NEW_WINDOW_ADDRESS}" ]] || {
		printf 'focus left spawned window %s (now %s); stopping synthetic input\n' \
			"${PARITY_NEW_WINDOW_ADDRESS}" "${active}" >&2
		return 1
	}
}

parity_type_loop() {
	local seconds=$1
	local end=$((SECONDS + seconds))
	while ((SECONDS < end)); do
		parity_focus_guard || return 1
		wtype -d 25 'parity scripted typing payload' -M ctrl -k u -m ctrl || return 1
		sleep 0.2
	done
}

# Scrollback is Shift+PageUp / Shift+PageDown in all three terminals (Freminal
# default binding, WezTerm and Ghostty defaults; the fixtures bind only
# F6-F9). Plain PageUp/PageDown is forwarded to the shell, which would leave
# the viewport at the bottom and measure nothing.
parity_scroll_loop() {
	local seconds=$1
	local end=$((SECONDS + seconds))
	while ((SECONDS < end)); do
		parity_focus_guard || return 1
		wtype -M shift -k Page_Up -m shift || return 1
		sleep 0.25
		parity_focus_guard || return 1
		wtype -M shift -k Page_Down -m shift || return 1
		sleep 0.25
	done
}

parity_setup_chrome_topology() {
	# Three new tabs, then a 2x2 split in the active (fourth) tab.
	wtype -k F6 -s 200 -k F6 -s 200 -k F6
	sleep 0.5
	wtype -k F7 -s 200 -k F8 -s 200 -k F9 -s 200 -k F8
}

# The shell line typed for sustained-output-marked. The shell, not wtype, turns
# the octal escapes into OSC 133 marks (\033 = ESC, \007 = BEL). Every 25th
# burst idles 0.15 s, longer than Freminal's 100 ms idle tick, so scrollback
# compression engages. The command id is the burst counter, so each burst opens
# and closes its own command block.
parity_marked_command() {
	printf '%s' "n=0; while :; do n=\$((n + 1)); printf '${PARITY_MARKED_PROMPT_FORMAT}' \"\$n\" \"\$n\" \"\$n\"; seq 1 200; printf '${PARITY_MARKED_FINISH_FORMAT}' \"\$n\"; if ((n % 25)); then sleep 0.02; else sleep 0.15; fi; done"
}

parity_start_workload() {
	local workload=$1 duration=$2 run_dir=$3
	case "${workload}" in
	idle-blink | idle-steady | chrome-blink | chrome-steady | pointer)
		;;
	typing)
		parity_type_loop "${duration}" &
		printf '%s\n' "$!"
		;;
	sparse-row)
		wtype "n=0; while :; do printf '\\rParity sparse row %08d\\033[K' \"\$((++n))\"; sleep 0.05; done" -k Return
		;;
	btop)
		local btop_bin btop_command
		btop_bin=$(parity_btop_bin) || return 1
		# %q quotes for the interactive bash that receives the typed text, so
		# a run directory containing quotes or spaces cannot break the line.
		printf -v btop_command '%q --config %q --force-utf --update 1000' \
			"${btop_bin}" "${run_dir}/btop.conf"
		wtype "${btop_command}" -k Return
		;;
	sustained-output)
		wtype "while :; do seq 1 200; sleep 0.02; done" -k Return
		;;
	sustained-output-marked)
		wtype "$(parity_marked_command)" -k Return
		;;
	sustained-output-varying)
		wtype "n=1; while :; do seq \$n \$((n + 199)); n=\$((n + 200)); sleep 0.02; done" -k Return
		;;
	streaming-output)
		wtype "n=0; while :; do printf 'Parity stream line %08d\\n' \"\$((++n))\"; sleep 0.02; done" -k Return
		;;
	*)
		printf 'unknown workload: %s\n' "${workload}" >&2
		return 2
		;;
	esac
}

parity_descendants_one() {
	local root=$1 child_file child
	printf '%s\n' "${root}"
	child_file="/proc/${root}/task/${root}/children"
	[[ -r "${child_file}" ]] || return 0
	for child in $(<"${child_file}"); do
		parity_descendants_one "${child}"
	done
}

parity_descendants() {
	parity_descendants_one "$1"
}

parity_terminate_tree() {
	local root=$1 expected_start=$2 pid current_start
	local -a pids=() live=()
	[[ -r "/proc/${root}/stat" ]] || return 0
	current_start=$(parity_process_start_time "${root}")
	if [[ ${current_start} != "${expected_start}" ]]; then
		printf 'refusing to signal reused PID %s\n' "${root}" >&2
		return 1
	fi
	mapfile -t pids < <(parity_descendants "${root}" | tac)
	((${#pids[@]} > 0)) || return 0
	live=()
	for pid in "${pids[@]}"; do
		kill -0 "${pid}" 2>/dev/null && live+=("${pid}")
	done
	((${#live[@]} == 0)) || kill -TERM "${live[@]}" 2>/dev/null || true
	for _ in {1..50}; do
		live=()
		for pid in "${pids[@]}"; do
			kill -0 "${pid}" 2>/dev/null && live+=("${pid}")
		done
		((${#live[@]} == 0)) && return 0
		sleep 0.1
	done
	live=()
	for pid in "${pids[@]}"; do
		kill -0 "${pid}" 2>/dev/null && live+=("${pid}")
	done
	((${#live[@]} == 0)) || kill -KILL "${live[@]}" 2>/dev/null || true
}

parity_process_start_time() {
	local pid=$1 stat
	local -a fields=()
	stat=$(<"/proc/${pid}/stat")
	stat=${stat##*) }
	read -ra fields <<<"${stat}"
	printf '%s\n' "${fields[19]}"
}
