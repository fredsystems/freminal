#!/run/current-system/sw/bin/bash
# Workload definitions shared by the Task 125 parity runner.

set -euo pipefail

readonly TASK125_SYSTEM_BASH="/run/current-system/sw/bin/bash"
TASK125_BTOP_BIN=$(command -v btop)
readonly TASK125_BTOP_BIN
declare -gx TASK125_NEW_WINDOW_ADDRESS=''
declare -gx TASK125_NEW_WINDOW_PID=''

task125_require_system_bash() {
	if [[ ! -x "${TASK125_SYSTEM_BASH}" ]]; then
		printf 'Task 125 requires %s (NixOS bash-interactive).\n' "${TASK125_SYSTEM_BASH}" >&2
		return 1
	fi
}

task125_replace_token() {
	local file=$1 token=$2 value=$3
	python3 - "${file}" "${token}" "${value}" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
token = f"@{sys.argv[2]}@"
path.write_text(path.read_text(encoding="utf-8").replace(token, sys.argv[3]), encoding="utf-8")
PY
}

task125_render_fixtures() {
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

	task125_replace_token "${run_dir}/shell.rc" TASK125_MARKER "${marker}"
	task125_replace_token "${run_dir}/freminal.toml" CURSOR_BLINK "${blink_bool}"
	task125_replace_token "${run_dir}/wezterm.lua" CURSOR_BLINK_STYLE "${cursor_style}"
	task125_replace_token "${run_dir}/wezterm.lua" SHELL_RC "${run_dir}/shell.rc"
	task125_replace_token "${run_dir}/ghostty.conf" CURSOR_STYLE_BLINK "${blink_bool}"
	task125_replace_token "${run_dir}/ghostty.conf" SHELL_RC "${run_dir}/shell.rc"

	unset TASK125_FORCE_TERMINAL_ROWS TASK125_FORCE_TERMINAL_COLS
}

task125_wait_focused() {
	local address=$1 active
	for _ in {1..50}; do
		active=$(hyprctl activewindow -j | jq -r '.address')
		[[ ${active} == "${address}" ]] && return 0
		sleep 0.1
	done
	printf 'new window %s did not become focused; refusing synthetic input\n' "${address}" >&2
	return 1
}

task125_pty_path() {
	local shell_pid=$1 target
	target=$(readlink "/proc/${shell_pid}/fd/0" 2>/dev/null || true)
	[[ ${target} == /dev/pts/* ]] && printf '%s\n' "${target}" && return 0
	printf 'shell PID %s has no PTY on fd 0\n' "${shell_pid}" >&2
	return 1
}

task125_wait_ready() {
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

task125_wait_new_window() {
	local baseline_file=$1 expected_class=$2 expected_config=$3 timeout_seconds=${4:-15}
	local address pid class
	local end=$((SECONDS + timeout_seconds))
	while ((SECONDS < end)); do
		while IFS=$'\t' read -r address pid class; do
			if [[ ${class} == "${expected_class}" ]] &&
				! grep -Fxq -- "${address}" "${baseline_file}" &&
				tr '\0' '\n' <"/proc/${pid}/environ" 2>/dev/null |
				grep -Fxq -- "XDG_CONFIG_HOME=${expected_config}"; then
				TASK125_NEW_WINDOW_ADDRESS=${address}
				TASK125_NEW_WINDOW_PID=${pid}
				return 0
			fi
		done < <(hyprctl clients -j | jq -r '.[] | [.address, (.pid | tostring), .class] | @tsv')
		sleep 0.1
	done
	printf 'timed out waiting for a newly mapped Hyprland client\n' >&2
	return 1
}

task125_type_loop() {
	local seconds=$1
	local end=$((SECONDS + seconds))
	while ((SECONDS < end)); do
		wtype -d 25 'task125 scripted typing payload' -M ctrl -k u -m ctrl
		sleep 0.2
	done
}

task125_scroll_loop() {
	local seconds=$1
	local end=$((SECONDS + seconds))
	while ((SECONDS < end)); do
		wtype -k Page_Up
		sleep 0.25
		wtype -k Page_Down
		sleep 0.25
	done
}

task125_setup_chrome_topology() {
	# Three new tabs, then a 2x2 split in the active (fourth) tab.
	wtype -k F6 -s 200 -k F6 -s 200 -k F6
	sleep 0.5
	wtype -k F7 -s 200 -k F8 -s 200 -k F9 -s 200 -k F8
}

task125_start_workload() {
	local workload=$1 duration=$2 run_dir=$3
	case "${workload}" in
	idle-blink | idle-steady | chrome-blink | chrome-steady | pointer)
		;;
	typing)
		task125_type_loop "${duration}" &
		printf '%s\n' "$!"
		;;
	sparse-row)
		wtype "n=0; while :; do printf '\\rTask125 sparse row %08d\\033[K' \"\$((++n))\"; sleep 0.05; done" -k Return
		;;
	btop)
		wtype "'${TASK125_BTOP_BIN}' --config '${run_dir}/btop.conf' --force-utf --update 1000" -k Return
		;;
	sustained-output)
		wtype "while :; do seq 1 200; sleep 0.02; done" -k Return
		;;
	*)
		printf 'unknown workload: %s\n' "${workload}" >&2
		return 2
		;;
	esac
}

task125_descendants_one() {
	local root=$1 child_file child
	printf '%s\n' "${root}"
	child_file="/proc/${root}/task/${root}/children"
	[[ -r "${child_file}" ]] || return 0
	for child in $(<"${child_file}"); do
		task125_descendants_one "${child}"
	done
}

task125_descendants() {
	task125_descendants_one "$1"
}

task125_terminate_tree() {
	local root=$1 expected_start=$2 pid current_start
	local -a pids=() live=()
	[[ -r "/proc/${root}/stat" ]] || return 0
	current_start=$(task125_process_start_time "${root}")
	if [[ ${current_start} != "${expected_start}" ]]; then
		printf 'refusing to signal reused PID %s\n' "${root}" >&2
		return 1
	fi
	mapfile -t pids < <(task125_descendants "${root}" | tac)
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

task125_process_start_time() {
	local pid=$1 stat
	local -a fields=()
	stat=$(<"/proc/${pid}/stat")
	stat=${stat##*) }
	read -ra fields <<<"${stat}"
	printf '%s\n' "${fields[19]}"
}
