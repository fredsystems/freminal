#!/run/current-system/sw/bin/bash
# Reproducible Task 125 Freminal/WezTerm/Ghostty parity capture driver.

set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
readonly SCRIPT_DIR
# shellcheck source=assets/profiling/task125/workloads.sh
source "${SCRIPT_DIR}/workloads.sh"

readonly WEZTERM_BIN="/nix/store/fjd3yyncgw5wj0vv8wkvlibp5z4wqqzg-wezterm-0-unstable-2026-08-12/bin/wezterm"
readonly GHOSTTY_BIN="/nix/store/ij9fvnhfj710aafmlav1psl434cw5wqc-ghostty-1.3.1/bin/ghostty"
readonly SCREEN_WARMUP=5
readonly SCREEN_DURATION=20
readonly SCREEN_REPEATS=3
readonly CONFIRM_WARMUP=10
readonly CONFIRM_DURATION=60
readonly CONFIRM_REPEATS=7
readonly AMDGPU_PCI="0000:03:00.0"
SAMPLE_WARMUP=${SCREEN_WARMUP}
SAMPLE_DURATION=${SCREEN_DURATION}
SAMPLE_REPEATS=${SCREEN_REPEATS}

declare -a OWNED_PIDS=()
declare -A OWNED_START_TIMES=()
ASSUME_READY=false
LAUNCHED_PID=

usage() {
	printf '%s\n' \
		'usage:' \
		'  run-matrix.sh preflight' \
		'  run-matrix.sh smoke TERMINAL CURSOR [--assume-ready]' \
		'  run-matrix.sh screen [OUTPUT_DIR] [--assume-ready]' \
		'  run-matrix.sh pointer-screen [OUTPUT_DIR]' \
		'  run-matrix.sh confirm OUTPUT_DIR WORKLOAD [WORKLOAD...] [--assume-ready]' \
		'' \
		'TERMINAL: freminal | wezterm | ghostty' \
		'CURSOR: blink | steady'
}

task125_tree_tids() {
	local root=$1 pid task
	while read -r pid; do
		for task in "/proc/${pid}"/task/*; do
			[[ -d ${task} ]] && printf '%s\n' "${task##*/}"
		done
	done < <(task125_descendants "${root}")
}

task125_pid_csv() {
	local root=$1
	task125_descendants "${root}" | paste -sd, -
}

task125_wakeup_filter() {
	local root=$1 tid filter=''
	while read -r tid; do
		if [[ -n ${filter} ]]; then
			filter+=' || '
		fi
		filter+="pid == ${tid}"
	done < <(task125_tree_tids "${root}")
	printf '%s\n' "${filter}"
}

task125_drm_gfx_ns() {
	local root=$1 pid file line client gfx key total=0
	local -A seen=()
	while read -r pid; do
		for file in "/proc/${pid}"/fdinfo/*; do
			[[ -r ${file} ]] || continue
			client='' gfx=''
			while IFS= read -r line; do
				case "${line}" in
				drm-client-id:*) client=${line#*:} ;;
				drm-engine-gfx:*) gfx=${line#*:} ;;
				esac
			done <"${file}"
			client=${client//[[:space:]]/}
			gfx=${gfx//[!0-9]/}
			[[ -n ${client} && -n ${gfx} ]] || continue
			# A DRM fd can be inherited by a child; client id, not PID+client,
			# is the identity to deduplicate across the complete process tree.
			key=${client}
			if [[ -z ${seen[${key}]:-} ]]; then
				seen[${key}]=1
				total=$((total + gfx))
			fi
		done
	done < <(task125_descendants "${root}")
	printf '%s\n' "${total}"
}

task125_perf_value() {
	local file=$1 event=$2
	awk -F, -v event="${event}" '$3 == event || $3 == event ":u" { gsub(/^[[:space:]]+|[[:space:]]+$/, "", $1); print $1; exit }' "${file}"
}

task125_tree_cpu_ticks() {
	local root=$1 pid stat
	local user=0 system=0
	local -a fields=()
	while read -r pid; do
		[[ -r "/proc/${pid}/stat" ]] || continue
		stat=$(<"/proc/${pid}/stat")
		stat=${stat##*) }
		read -ra fields <<<"${stat}"
		user=$((user + fields[11]))
		system=$((system + fields[12]))
	done < <(task125_descendants "${root}")
	printf '%s %s\n' "${user}" "${system}"
}

task125_expected_render_node() {
	local node
	for node in "/sys/bus/pci/devices/${AMDGPU_PCI}/drm"/renderD*; do
		[[ -e ${node} ]] && printf '/dev/dri/%s\n' "${node##*/}" && return 0
	done
	printf 'no DRM render node for %s\n' "${AMDGPU_PCI}" >&2
	return 1
}

task125_verify_process_gpu() {
	local root=$1 expected pid fd target
	expected=$(task125_expected_render_node)
	while read -r pid; do
		for fd in "/proc/${pid}"/fd/*; do
			[[ -e ${fd} ]] || continue
			target=$(readlink "${fd}" 2>/dev/null || true)
			[[ ${target} == "${expected}" ]] && return 0
		done
	done < <(task125_descendants "${root}")
	printf 'process tree %s has no open fd on %s; refusing GPU measurement\n' \
		"${root}" "${expected}" >&2
	return 1
}

record_metadata() {
	local output_dir=$1
	{
		printf 'captured_at=%s\n' "$(date --iso-8601=seconds)"
		printf 'git_commit=%s\n' "$(git rev-parse HEAD)"
		printf 'wezterm_bin=%s\n' "${WEZTERM_BIN}"
		"${WEZTERM_BIN}" --version
		printf 'ghostty_bin=%s\n' "${GHOSTTY_BIN}"
		"${GHOSTTY_BIN}" --version
		uname -a
		hyprctl version
		hyprctl monitors -j
		amdgpu_top --list
		lspci -nnk
	} >"${output_dir}/environment.txt"
}

collector_preflight() {
	printf 'Scheduler wakeup collection needs one sudo authentication before any windows spawn.\n'
	sudo -v
	sudo test -r /sys/kernel/tracing/events/sched/sched_wakeup/format
	perf stat -x, -e task-clock,cycles,instructions,context-switches -- true \
		>/dev/null 2>&1
}

collector_overhead_control() {
	local output_dir=$1
	perf stat -x, -o "${output_dir}/amdgpu-top-overhead.perf.csv" \
		-- amdgpu_top --json --process --no-pc --pci "${AMDGPU_PCI}" -s 100ms -n 20 \
		>"${output_dir}/amdgpu-top-overhead.json"
}

capture_collectors() {
	local root=$1 duration=$2 run_dir=$3
	local pids filter gpu_before gpu_after cpu_before cpu_after perf_pid wake_pid
	local user_before system_before user_after system_after
	pids=$(task125_pid_csv "${root}")
	filter=$(task125_wakeup_filter "${root}")
	[[ -n ${pids} && -n ${filter} ]] || {
		printf 'empty process tree for collector root %s\n' "${root}" >&2
		return 1
	}
	amdgpu_top --json --process --no-pc --pci "${AMDGPU_PCI}" -n 1 \
		>"${run_dir}/amdgpu-processes.json"
	gpu_before=$(task125_drm_gfx_ns "${root}")
	cpu_before=$(task125_tree_cpu_ticks "${root}")

	perf stat -x, -o "${run_dir}/perf.csv" \
		-e task-clock,cycles,instructions,context-switches \
		-p "${pids}" --timeout "$((duration * 1000))" &
	perf_pid=$!
	sudo perf stat -x, --log-fd 3 -a \
		-e "sched:sched_wakeup/${filter}/" --timeout "$((duration * 1000))" \
		3>"${run_dir}/wakeups.csv" &
	wake_pid=$!
	wait "${perf_pid}"
	wait "${wake_pid}"
	cpu_after=$(task125_tree_cpu_ticks "${root}")
	gpu_after=$(task125_drm_gfx_ns "${root}")
	printf '%s\n' "$((gpu_after - gpu_before))" >"${run_dir}/gpu-gfx-ns"
	read -r user_before system_before <<<"${cpu_before}"
	read -r user_after system_after <<<"${cpu_after}"
	printf '%s %s\n' "$((user_after - user_before))" "$((system_after - system_before))" \
		>"${run_dir}/cpu-ticks"
}

append_sample_csv() {
	local csv=$1 repeat=$2 workload=$3 terminal=$4 run_dir=$5
	local task_clock user_ticks system_ticks user_clock kernel_clock cycles instructions switches wakeups gpu clock_tick
	task_clock=$(task125_perf_value "${run_dir}/perf.csv" task-clock)
	read -r user_ticks system_ticks <"${run_dir}/cpu-ticks"
	clock_tick=$(getconf CLK_TCK)
	user_clock=$(awk -v ticks="${user_ticks}" -v hz="${clock_tick}" 'BEGIN { printf "%.6f", ticks * 1000 / hz }')
	kernel_clock=$(awk -v ticks="${system_ticks}" -v hz="${clock_tick}" 'BEGIN { printf "%.6f", ticks * 1000 / hz }')
	cycles=$(task125_perf_value "${run_dir}/perf.csv" cycles)
	instructions=$(task125_perf_value "${run_dir}/perf.csv" instructions)
	switches=$(task125_perf_value "${run_dir}/perf.csv" context-switches)
	wakeups=$(task125_perf_value "${run_dir}/wakeups.csv" sched:sched_wakeup)
	gpu=$(<"${run_dir}/gpu-gfx-ns")
	printf '%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n' \
		"${repeat}" "${workload}" "${terminal}" "${SAMPLE_DURATION}" \
		"${task_clock}" "${user_clock}" "${kernel_clock}" "${cycles}" \
		"${instructions}" "${switches}" "${wakeups}" "${gpu}" >>"${csv}"
}

cleanup() {
	local pid start
	for pid in "${OWNED_PIDS[@]}"; do
		start=${OWNED_START_TIMES[${pid}]:-}
		[[ -n ${start} ]] && task125_terminate_tree "${pid}" "${start}"
	done
}
trap cleanup EXIT INT TERM

require_command() {
	command -v "$1" >/dev/null || {
		printf 'missing required command: %s\n' "$1" >&2
		return 1
	}
}

preflight() {
	local command
	task125_require_system_bash
	for command in perf wtype amdgpu_top hyprctl python3 jq install stat tac \
		paste awk tee realpath date lspci readlink getconf; do
		require_command "${command}"
	done
	[[ -x "${WEZTERM_BIN}" ]] || {
		printf 'missing pinned WezTerm\n' >&2
		return 1
	}
	[[ -x "${GHOSTTY_BIN}" ]] || {
		printf 'missing pinned Ghostty\n' >&2
		return 1
	}
	[[ -n "${WAYLAND_DISPLAY:-}" && -n "${XDG_RUNTIME_DIR:-}" ]] || {
		printf 'Wayland session variables are unavailable\n' >&2
		return 1
	}
	if [[ ${LIBGL_ALWAYS_SOFTWARE:-} == 1 ]]; then
		printf 'LIBGL_ALWAYS_SOFTWARE=1; leave the gl-pixel shell before measuring\n' >&2
		return 1
	fi
	if [[ ! -x "${FREMINAL_BIN:-${PWD}/target/release/freminal}" ]]; then
		printf 'Set FREMINAL_BIN or build target/release/freminal before a GUI run.\n' >&2
		return 1
	fi
	printf 'Tracefs access is intentionally root-only; measurement capture will request sudo -v.\n'
	printf 'Preflight passed without spawning a window.\n'
}

interaction_notice() {
	local terminal=$1 cursor=$2 workload=${3:-smoke}
	printf '\nTask 125 is about to spawn one %s window (%s cursor, workload=%s).\n' \
		"${terminal}" "${cursor}" "${workload}"
	printf '%s\n' \
		'Hyprland may place it under the current pointer; startup pointer events are expected' \
		"and discarded during the ${SAMPLE_WARMUP}-second warm-up."
	if [[ ${workload} == pointer ]]; then
		printf '%s\n' 'Interaction required: after warm-up, move the physical pointer continuously over inert terminal content until told to stop.'
	else
		printf '%s\n' 'Interaction required: none. Leave the window focused; do not type, click, resize, or move the pointer over it.'
	fi
	if [[ ${ASSUME_READY} == false ]]; then
		read -r -p 'Press Enter when the desktop is ready, or Ctrl-C to cancel. '
	fi
}

isolated_env() {
	local run_dir=$1
	shift
	env -i \
		HOME="${run_dir}/home" \
		PATH="/run/current-system/sw/bin" \
		USER="${USER}" LOGNAME="${LOGNAME:-${USER}}" \
		LANG=C.UTF-8 LC_ALL=C.UTF-8 \
		SHELL="${TASK125_SYSTEM_BASH}" \
		XDG_CONFIG_HOME="${run_dir}/config" \
		XDG_CACHE_HOME="${run_dir}/cache" \
		XDG_STATE_HOME="${run_dir}/state" \
		XDG_DATA_HOME="${run_dir}/data" \
		XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR}" \
		WAYLAND_DISPLAY="${WAYLAND_DISPLAY}" \
		XDG_SESSION_TYPE="${XDG_SESSION_TYPE:-wayland}" \
		DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-}" \
		DISPLAY="${DISPLAY:-}" \
		LD_LIBRARY_PATH="${LD_LIBRARY_PATH:-}" \
		TASK125_READY_FILE="${run_dir}/ready" \
		TASK125_GRID_REPORT_FILE="${run_dir}/grid" \
		TASK125_EXPECTED_ROWS="${TASK125_ROWS}" \
		TASK125_EXPECTED_COLS="${TASK125_COLS}" \
		TASK125_FORCE_TERMINAL_ROWS="${TASK125_FORCE_TERMINAL_ROWS:-}" \
		TASK125_FORCE_TERMINAL_COLS="${TASK125_FORCE_TERMINAL_COLS:-}" \
		"$@"
}

launch_terminal() {
	local terminal=$1 cursor=$2 marker=$3 run_dir=$4
	local freminal_bin=${FREMINAL_BIN:-${PWD}/target/release/freminal} pty
	mkdir -p "${run_dir}"/{home,config,cache,state,data}
	task125_render_fixtures "${SCRIPT_DIR}" "${run_dir}" "${terminal}" "${cursor}" "${marker}"
	if [[ ${terminal} == freminal ]]; then
		# A fresh isolated XDG state otherwise opens the first-run welcome
		# overlay, contaminating every sample. Seed only the persisted onboarding
		# bit; all other application state remains fresh for each run.
		mkdir -p "${run_dir}/state/freminal"
		printf 'first_run_complete = true\n' >"${run_dir}/state/freminal/state.toml"
	fi
	case "${terminal}" in
	freminal)
		isolated_env "${run_dir}" "${freminal_bin}" --config "${run_dir}/freminal.toml" -- \
			"${TASK125_SYSTEM_BASH}" --noprofile --rcfile "${run_dir}/shell.rc" -i &
		;;
	wezterm)
		isolated_env "${run_dir}" "${WEZTERM_BIN}" --config-file "${run_dir}/wezterm.lua" start --always-new-process &
		;;
	ghostty)
		isolated_env "${run_dir}" "${GHOSTTY_BIN}" --config-default-files=false --config-file="${run_dir}/ghostty.conf" &
		;;
	*)
		printf 'unknown terminal: %s\n' "${terminal}" >&2
		return 2
		;;
	esac
	local pid=$! start
	start=$(task125_process_start_time "${pid}")
	OWNED_PIDS+=("${pid}")
	OWNED_START_TIMES[${pid}]=${start}
	task125_wait_ready "${run_dir}/ready" "${pid}"
	task125_focus_pid "${pid}"
	pty=$(task125_pty_path "${pid}")
	stty -F "${pty}" size >"${run_dir}/grid"
	LAUNCHED_PID=${pid}
}

verify_grid() {
	local run_dir=$1 grid
	grid=$(<"${run_dir}/grid")
	if [[ ${grid} != "${TASK125_ROWS} ${TASK125_COLS}" ]]; then
		printf 'grid mismatch: wanted %sx%s, got %s\n' "${TASK125_COLS}" "${TASK125_ROWS}" "${grid}" >&2
		return 1
	fi
}

smoke() {
	local terminal=$1 cursor=$2
	interaction_notice "${terminal}" "${cursor}" smoke
	local run_dir pid
	run_dir=$(mktemp -d "${TMPDIR:-/tmp}/freminal-task125-smoke.XXXXXX")
	launch_terminal "${terminal}" "${cursor}" "smoke-${terminal}" "${run_dir}"
	pid=${LAUNCHED_PID}
	sleep "${SAMPLE_WARMUP}"
	verify_grid "${run_dir}"
	task125_verify_process_gpu "${pid}"
	printf 'Smoke window is correctly sized and will close in 5 seconds.\n'
	sleep 5
	task125_terminate_tree "${pid}" "${OWNED_START_TIMES[${pid}]}"
}

cursor_for_workload() {
	case "$1" in
	idle-steady | chrome-steady) printf 'steady\n' ;;
	*) printf 'blink\n' ;;
	esac
}

prepare_workload_before_warmup() {
	local workload=$1 run_dir=$2
	case "${workload}" in
	chrome-blink | chrome-steady)
		task125_setup_chrome_topology
		;;
	btop | sparse-row | sustained-output)
		task125_start_workload "${workload}" "${SAMPLE_DURATION}" "${run_dir}" >/dev/null
		;;
	scrollback)
		wtype 'seq 1 10000' -k Return
		;;
	esac
}

start_workload_after_warmup() {
	local workload=$1 run_dir=$2
	case "${workload}" in
	typing)
		task125_type_loop "${SAMPLE_DURATION}" &
		;;
	scrollback)
		task125_scroll_loop "${SAMPLE_DURATION}" &
		;;
	pointer)
		printf '%s\n' \
			'Pointer capture is ready.' \
			"Move the physical pointer continuously over inert terminal content for ${SAMPLE_DURATION} seconds."
		read -r -p 'Press Enter to start the pointer interval. '
		;;
	esac
}

capture_one() {
	local output_dir=$1 repeat=$2 workload=$3 terminal=$4 csv=$5
	local cursor run_dir pid
	cursor=$(cursor_for_workload "${workload}")
	interaction_notice "${terminal}" "${cursor}" "${workload}"
	run_dir="${output_dir}/raw/${workload}/repeat-${repeat}/${terminal}"
	mkdir -p "${run_dir}"
	launch_terminal "${terminal}" "${cursor}" "${workload}-${repeat}-${terminal}" "${run_dir}"
	pid=${LAUNCHED_PID}
	prepare_workload_before_warmup "${workload}" "${run_dir}"
	sleep "${SAMPLE_WARMUP}"
	verify_grid "${run_dir}"
	start_workload_after_warmup "${workload}" "${run_dir}"
	capture_collectors "${pid}" "${SAMPLE_DURATION}" "${run_dir}"
	append_sample_csv "${csv}" "${repeat}" "${workload}" "${terminal}" "${run_dir}"
	task125_terminate_tree "${pid}" "${OWNED_START_TIMES[${pid}]}"
}

matrix_output_dir() {
	local arg
	for arg in "$@"; do
		if [[ ${arg} != --assume-ready ]]; then
			printf '%s\n' "${arg}"
			return 0
		fi
	done
	printf '%s/freminal-task125-%(%Y%m%d-%H%M%S)T\n' "${TMPDIR:-/tmp}" -1
}

run_series() {
	local label=$1 output_dir=$2
	shift 2
	local -a workloads=("$@")
	local csv repeat workload terminal offset index window_count seconds
	case "$(realpath -m "${output_dir}")" in
	"${PWD}" | "${PWD}"/*)
		printf 'raw capture output must be outside the repository: %s\n' "${output_dir}" >&2
		return 2
		;;
	esac
	mkdir -p "${output_dir}/raw"
	csv="${output_dir}/${label}-samples.csv"
	printf '%s\n' 'repeat,workload,terminal,wall_seconds,task_clock_ms,user_task_clock_ms,kernel_task_clock_ms,cycles,instructions,context_switches,wakeups,gpu_gfx_ns' >"${csv}"

	window_count=$((SAMPLE_REPEATS * ${#workloads[@]} * 3))
	seconds=$((window_count * (SAMPLE_WARMUP + SAMPLE_DURATION)))
	printf '%s\n' \
		"This ${label} pass spawns ${window_count} sequential windows and takes about $((seconds / 60)) minutes." \
		'All non-pointer windows must remain focused and untouched.' \
		"Pointer runs pause after warm-up and request ${SAMPLE_DURATION} seconds of physical movement."
	if [[ ${ASSUME_READY} == false ]]; then
		read -r -p 'Press Enter to authorize the complete matrix, or Ctrl-C to cancel. '
		ASSUME_READY=true
	fi

	collector_preflight
	record_metadata "${output_dir}"
	collector_overhead_control "${output_dir}"
	local -a terminals=(freminal wezterm ghostty)
	for ((repeat = 1; repeat <= SAMPLE_REPEATS; repeat++)); do
		offset=$(((repeat - 1) % ${#terminals[@]}))
		for workload in "${workloads[@]}"; do
			for ((index = 0; index < ${#terminals[@]}; index++)); do
				terminal=${terminals[$(((index + offset) % ${#terminals[@]}))]}
				capture_one "${output_dir}" "${repeat}" "${workload}" "${terminal}" "${csv}"
			done
		done
	done
	python3 "${SCRIPT_DIR}/summarize.py" --expected-repeats "${SAMPLE_REPEATS}" "${csv}" |
		tee "${output_dir}/${label}-summary.txt"
	printf 'Task 125 raw results: %s\n' "${output_dir}"
}

run_screen() {
	local output_dir
	output_dir=$(matrix_output_dir "$@")
	SAMPLE_WARMUP=${SCREEN_WARMUP}
	SAMPLE_DURATION=${SCREEN_DURATION}
	SAMPLE_REPEATS=${SCREEN_REPEATS}
	run_series screen "${output_dir}" \
		idle-blink idle-steady typing sparse-row btop scrollback sustained-output \
		chrome-blink chrome-steady
}

run_pointer_screen() {
	local output_dir
	output_dir=$(matrix_output_dir "$@")
	SAMPLE_WARMUP=${SCREEN_WARMUP}
	SAMPLE_DURATION=${SCREEN_DURATION}
	SAMPLE_REPEATS=${SCREEN_REPEATS}
	run_series pointer-screen "${output_dir}" pointer
}

run_confirmation() {
	local output_dir=$1
	shift
	local -a workloads=()
	local arg
	for arg in "$@"; do
		[[ ${arg} == --assume-ready ]] || workloads+=("${arg}")
	done
	((${#workloads[@]} > 0)) || {
		printf 'confirmation requires at least one workload\n' >&2
		return 2
	}
	SAMPLE_WARMUP=${CONFIRM_WARMUP}
	SAMPLE_DURATION=${CONFIRM_DURATION}
	SAMPLE_REPEATS=${CONFIRM_REPEATS}
	run_series confirm "${output_dir}" "${workloads[@]}"
}

parse_args() {
	local arg
	for arg in "$@"; do
		[[ ${arg} == --assume-ready ]] && ASSUME_READY=true
	done
}

main() {
	(($# > 0)) || {
		usage
		return 2
	}
	local command=$1
	shift
	parse_args "$@"
	case "${command}" in
	preflight)
		preflight
		;;
	smoke)
		(($# >= 2)) || {
			usage
			return 2
		}
		smoke "$1" "$2"
		;;
	screen)
		run_screen "$@"
		;;
	pointer-screen)
		run_pointer_screen "$@"
		;;
	confirm)
		(($# >= 2)) || {
			usage
			return 2
		}
		run_confirmation "$@"
		;;
	*)
		usage
		return 2
		;;
	esac
}

main "$@"
