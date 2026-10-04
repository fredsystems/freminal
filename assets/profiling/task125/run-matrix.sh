#!/run/current-system/sw/bin/bash
# Reproducible Task 125 Freminal/WezTerm/Ghostty parity capture driver.

set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
readonly SCRIPT_DIR
# shellcheck source=assets/profiling/task125/workloads.sh
source "${SCRIPT_DIR}/workloads.sh"

readonly WEZTERM_BIN="/nix/store/mmvpz8sgpp4gg1lwsjv68knvkzqmsdwk-wezterm-0-unstable-2026-09-17/bin/wezterm"
readonly GHOSTTY_BIN="/nix/store/i5zqr903i6yb642h9amwh5174n5bmfc4-ghostty-1.3.1/bin/ghostty"
readonly SCREEN_WARMUP=5
readonly SCREEN_DURATION=20
readonly SCREEN_REPEATS=3
readonly CONFIRM_WARMUP=10
readonly CONFIRM_DURATION=60
readonly CONFIRM_REPEATS=7
readonly AMDGPU_PCI="0000:03:00.0"
readonly TASK125_FREMINAL_RUST_LOG='none,freminal::frame_profiling=debug,freminal::task_125::live_render_profile=debug,freminal_windowing::frame_profiling=debug,freminal_windowing::gl_context=info,freminal::task_125::gpu_timing=debug,freminal_windowing::task_125::gpu_timing=debug'
SAMPLE_WARMUP=${SCREEN_WARMUP}
SAMPLE_DURATION=${SCREEN_DURATION}
SAMPLE_REPEATS=${SCREEN_REPEATS}
GEOMETRY_BASELINE_FILE=
GEOMETRY_BASELINE_INITIALIZED=false

declare -a OWNED_PIDS=()
declare -A OWNED_START_TIMES=()
ASSUME_READY=false
LAUNCHED_PID=
LAUNCHED_SHELL_PID=

usage() {
	printf '%s\n' \
		'usage:' \
		'  run-matrix.sh preflight' \
		'  run-matrix.sh collector-preflight' \
		'  run-matrix.sh smoke TERMINAL CURSOR [--assume-ready]' \
		'  run-matrix.sh profile-smoke [--assume-ready]' \
		'  run-matrix.sh collector-smoke-one TERMINAL [OUTPUT_DIR] [--assume-ready]' \
		'  run-matrix.sh collector-smoke [OUTPUT_DIR] [--assume-ready]' \
		'  run-matrix.sh screen [OUTPUT_DIR] [--assume-ready]' \
		'  run-matrix.sh pointer-screen [OUTPUT_DIR]' \
		'  run-matrix.sh confirm OUTPUT_DIR WORKLOAD [WORKLOAD...] [--assume-ready]' \
		'' \
		'TERMINAL: freminal | wezterm | ghostty' \
		'CURSOR: blink | steady'
}

task125_tree_tids() {
	local root=$1 task
	for task in "/proc/${root}"/task/*; do
		[[ -d ${task} ]] && printf '%s\n' "${task##*/}"
	done
}

task125_pid_csv() {
	local root=$1
	printf '%s\n' "${root}"
}

task125_wakeup_filter() {
	local tids_file=$1 tid filter=''
	while read -r tid; do
		if [[ -n ${filter} ]]; then
			filter+=' || '
		fi
		filter+="pid == ${tid}"
	done <"${tids_file}"
	printf '%s\n' "${filter}"
}

task125_drm_gfx_ns() {
	local root=$1 file fd target expected line client gfx key total=0
	local -A seen=()
	expected=$(task125_expected_render_node)
	for file in "/proc/${root}"/fdinfo/*; do
		[[ -r ${file} ]] || continue
		fd=${file##*/}
		target=$(readlink "/proc/${root}/fd/${fd}" 2>/dev/null || true)
		[[ ${target} == "${expected}" ]] || continue
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
		key=${client}
		if [[ -z ${seen[${key}]:-} ]]; then
			seen[${key}]=1
			total=$((total + gfx))
		fi
	done
	printf '%s\n' "${total}"
}

task125_perf_value() {
	local file=$1 event=$2
	awk -F, -v event="${event}" '$3 == event || $3 == event ":u" { gsub(/^[[:space:]]+|[[:space:]]+$/, "", $1); print $1; exit }' "${file}"
}

task125_tree_cpu_ticks() {
	local root=$1 stat
	local -a fields=()
	stat=$(<"/proc/${root}/stat")
	stat=${stat##*) }
	read -ra fields <<<"${stat}"
	printf '%s %s\n' "${fields[11]}" "${fields[12]}"
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
	local root=$1 expected fd target
	expected=$(task125_expected_render_node)
	for fd in "/proc/${root}"/fd/*; do
		[[ -e ${fd} ]] || continue
		target=$(readlink "${fd}" 2>/dev/null || true)
		[[ ${target} == "${expected}" ]] && return 0
	done
	printf 'terminal GUI process %s has no open fd on %s; GPU metric unavailable\n' \
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
	sudo perf stat -x, -a -e sched:sched_wakeup --filter 'pid == 1' \
		--timeout 100 2>/dev/null
	sudo perf stat -x, -p $$ -e context-switches --timeout 100 2>/dev/null
	perf stat -x, -e task-clock,cycles,instructions -- true \
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
	local pids filter gpu_before gpu_after cpu_before cpu_after perf_pid wake_pid context_pid
	local user_before system_before user_after system_after
	printf 'process-tree\n' >"${run_dir}/collector-stage"
	pids=$(task125_pid_csv "${root}")
	task125_tree_tids "${root}" >"${run_dir}/tids-before"
	filter=$(task125_wakeup_filter "${run_dir}/tids-before")
	[[ -n ${pids} && -n ${filter} ]] || {
		printf 'empty process tree for collector root %s\n' "${root}" >&2
		return 1
	}
	printf 'gpu-discovery\n' >"${run_dir}/collector-stage"
	amdgpu_top --json --process --no-pc --pci "${AMDGPU_PCI}" -n 1 \
		>"${run_dir}/amdgpu-processes.json"
	gpu_before=$(task125_drm_gfx_ns "${root}")
	cpu_before=$(task125_tree_cpu_ticks "${root}")
	printf 'counters\n' >"${run_dir}/collector-stage"

	perf stat -x, -o "${run_dir}/perf.csv" \
		-e task-clock,cycles,instructions \
		-p "${pids}" --timeout "$((duration * 1000))" &
	perf_pid=$!
	sudo perf stat -x, -p "${root}" -e context-switches \
		--timeout "$((duration * 1000))" 2>"${run_dir}/context-switches.csv" &
	context_pid=$!
	sudo perf stat -x, -a -e sched:sched_wakeup --filter "${filter}" \
		--timeout "$((duration * 1000))" 2>"${run_dir}/wakeups.csv" &
	wake_pid=$!
	wait "${perf_pid}"
	wait "${context_pid}"
	wait "${wake_pid}"
	task125_tree_tids "${root}" >"${run_dir}/tids-after"
	# Threads that exit mid-capture were in the wakeup filter from the start,
	# and their CPU time folds into the process, so exits are recorded but do
	# not invalidate the sample. A new TID would be missing from the filter.
	if [[ -n $(comm -13 <(sort "${run_dir}/tids-before") <(sort "${run_dir}/tids-after")) ]]; then
		printf 'terminal GUI thread created during capture; sample is invalid\n' >&2
		printf 'invalid-thread-set\n' >"${run_dir}/collector-stage"
		return 1
	fi
	comm -23 <(sort "${run_dir}/tids-before") <(sort "${run_dir}/tids-after") | wc -l \
		>"${run_dir}/exited-tids"
	cpu_after=$(task125_tree_cpu_ticks "${root}")
	gpu_after=$(task125_drm_gfx_ns "${root}")
	printf '%s\n' "$((gpu_after - gpu_before))" >"${run_dir}/gpu-gfx-ns"
	read -r user_before system_before <<<"${cpu_before}"
	read -r user_after system_after <<<"${cpu_after}"
	printf '%s %s\n' "$((user_after - user_before))" "$((system_after - system_before))" \
		>"${run_dir}/cpu-ticks"
	printf 'complete\n' >"${run_dir}/collector-stage"
}

append_sample_csv() {
	local csv=$1 repeat=$2 workload=$3 terminal=$4 run_dir=$5
	local task_clock user_ticks system_ticks user_clock kernel_clock cycles instructions switches wakeups gpu clock_tick
	local grid_rows grid_cols
	task_clock=$(task125_perf_value "${run_dir}/perf.csv" task-clock)
	read -r user_ticks system_ticks <"${run_dir}/cpu-ticks"
	clock_tick=$(getconf CLK_TCK)
	user_clock=$(awk -v ticks="${user_ticks}" -v hz="${clock_tick}" 'BEGIN { printf "%.6f", ticks * 1000 / hz }')
	kernel_clock=$(awk -v ticks="${system_ticks}" -v hz="${clock_tick}" 'BEGIN { printf "%.6f", ticks * 1000 / hz }')
	cycles=$(task125_perf_value "${run_dir}/perf.csv" cycles)
	instructions=$(task125_perf_value "${run_dir}/perf.csv" instructions)
	switches=$(task125_perf_value "${run_dir}/context-switches.csv" context-switches)
	wakeups=$(task125_perf_value "${run_dir}/wakeups.csv" sched:sched_wakeup)
	gpu=$(<"${run_dir}/gpu-gfx-ns")
	read -r grid_rows grid_cols <"${run_dir}/grid"
	printf '%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n' \
		"${repeat}" "${workload}" "${terminal}" "${SAMPLE_DURATION}" \
		"${task_clock}" "${user_clock}" "${kernel_clock}" "${cycles}" \
		"${instructions}" "${switches}" "${wakeups}" "${gpu}" \
		"${grid_rows}" "${grid_cols}" >>"${csv}"
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
		paste awk tee realpath date lspci readlink getconf grep cmp tr; do
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
		printf 'profile-smoke additionally requires a build with --features frame-profiling,gpu-profiling.\n' >&2
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
		TASK125_SHELL_PID_FILE="${run_dir}/shell-pid" \
		TASK125_GRID_REPORT_FILE="${run_dir}/grid" \
		TASK125_FORCE_TERMINAL_ROWS="${TASK125_FORCE_TERMINAL_ROWS:-}" \
		TASK125_FORCE_TERMINAL_COLS="${TASK125_FORCE_TERMINAL_COLS:-}" \
		"$@"
}

launch_terminal() {
	local terminal=$1 cursor=$2 marker=$3 run_dir=$4
	local freminal_bin=${FREMINAL_BIN:-${PWD}/target/release/freminal}
	local pty pid start shell_pid shell_start expected_class expected_exe_root mapped_exe
	mkdir -p "${run_dir}"/{home,config,cache,state,data}
	hyprctl clients -j | jq -r '.[].address' >"${run_dir}/preexisting-windows"
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
		expected_class=freminal
		expected_exe_root=$(realpath -m "${freminal_bin}")
		isolated_env "${run_dir}" env RUST_LOG="${TASK125_FREMINAL_RUST_LOG}" \
			"${freminal_bin}" --config "${run_dir}/freminal.toml" -- \
			"${TASK125_SYSTEM_BASH}" --noprofile --rcfile "${run_dir}/shell.rc" -i \
			>"${run_dir}/freminal.stdout.log" 2>"${run_dir}/freminal.stderr.log" &
		;;
	wezterm)
		expected_class=org.wezfurlong.wezterm
		expected_exe_root=${WEZTERM_BIN%/bin/*}
		isolated_env "${run_dir}" "${WEZTERM_BIN}" --config-file "${run_dir}/wezterm.lua" start --always-new-process &
		;;
	ghostty)
		expected_class=com.mitchellh.ghostty
		expected_exe_root=${GHOSTTY_BIN%/bin/*}
		isolated_env "${run_dir}" "${GHOSTTY_BIN}" --config-default-files=false --config-file="${run_dir}/ghostty.conf" &
		;;
	*)
		printf 'unknown terminal: %s\n' "${terminal}" >&2
		return 2
		;;
	esac
	task125_wait_new_window \
		"${run_dir}/preexisting-windows" "${expected_class}" "${run_dir}/config"
	pid=${TASK125_NEW_WINDOW_PID}
	mapped_exe=$(readlink -f "/proc/${pid}/exe")
	if [[ ${terminal} == freminal ]]; then
		[[ ${mapped_exe} == "${expected_exe_root}" ]] || {
			printf 'mapped Freminal PID %s has unexpected executable %s\n' "${pid}" "${mapped_exe}" >&2
			return 1
		}
	elif [[ ${mapped_exe} != "${expected_exe_root}"/* ]]; then
		printf 'mapped %s PID %s is outside pinned store path: %s\n' \
			"${terminal}" "${pid}" "${mapped_exe}" >&2
		return 1
	fi
	start=$(task125_process_start_time "${pid}")
	OWNED_PIDS+=("${pid}")
	OWNED_START_TIMES[${pid}]=${start}
	task125_wait_ready "${run_dir}/ready" "${pid}"
	shell_pid=$(<"${run_dir}/shell-pid")
	shell_start=$(task125_process_start_time "${shell_pid}")
	OWNED_START_TIMES[${shell_pid}]=${shell_start}
	if ! task125_descendants_one "${pid}" | grep -Fxq -- "${shell_pid}"; then
		OWNED_PIDS+=("${shell_pid}")
	fi
	task125_wait_focused "${TASK125_NEW_WINDOW_ADDRESS}"
	hyprctl clients -j | jq --arg address "${TASK125_NEW_WINDOW_ADDRESS}" \
		'.[] | select(.address == $address)' >"${run_dir}/hyprland-client.json"
	pty=$(task125_pty_path "${shell_pid}")
	stty -F "${pty}" size >"${run_dir}/grid"
	LAUNCHED_PID=${pid}
	LAUNCHED_SHELL_PID=${shell_pid}
}

verify_grid() {
	local run_dir=$1 grid rows cols
	grid=$(<"${run_dir}/grid")
	read -r rows cols <<<"${grid}"
	if ((rows <= 0 || cols <= 0)); then
		printf 'invalid PTY grid: %s\n' "${grid}" >&2
		return 1
	fi
	printf 'Observed PTY grid: %sx%s\n' "${cols}" "${rows}"
}

ensure_external_output() {
	local output_dir=$1
	case "$(realpath -m "${output_dir}")" in
	"${PWD}" | "${PWD}"/*)
		printf 'raw capture output must be outside the repository: %s\n' "${output_dir}" >&2
		return 2
		;;
	esac
}

verify_series_geometry() {
	local run_dir=$1 current
	[[ -n ${GEOMETRY_BASELINE_FILE} ]] || return 0
	current="${run_dir}/hyprland-geometry.tsv"
	jq -r '[.monitor, .workspace.id, .at[0], .at[1], .size[0], .size[1]] | @tsv' \
		"${run_dir}/hyprland-client.json" >"${current}"
	if [[ ${GEOMETRY_BASELINE_INITIALIZED} == false ]]; then
		cp "${current}" "${GEOMETRY_BASELINE_FILE}"
		GEOMETRY_BASELINE_INITIALIZED=true
	elif ! cmp -s "${current}" "${GEOMETRY_BASELINE_FILE}"; then
		printf 'Hyprland geometry differs from the first sample; refusing incomparable capture\n' >&2
		printf 'expected: %s\nactual:   %s\n' \
			"$(<"${GEOMETRY_BASELINE_FILE}")" "$(<"${current}")" >&2
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
	if task125_verify_process_gpu "${pid}"; then
		printf 'available\n' >"${run_dir}/gpu-process-status"
	else
		printf 'unavailable\n' >"${run_dir}/gpu-process-status"
	fi
	if [[ ${terminal} == freminal ]]; then
		printf 'Freminal profiling output: %s\n' "${run_dir}/freminal.stdout.log"
	fi
	printf 'Smoke window is correctly sized and will close in 5 seconds.\n'
	sleep 5
	task125_terminate_tree "${pid}" "${OWNED_START_TIMES[${pid}]}"
	if [[ ${LAUNCHED_SHELL_PID} != "${pid}" ]]; then
		task125_terminate_tree \
			"${LAUNCHED_SHELL_PID}" "${OWNED_START_TIMES[${LAUNCHED_SHELL_PID}]}"
	fi
}

profile_smoke() {
	SAMPLE_WARMUP=5
	SAMPLE_DURATION=10
	interaction_notice freminal blink sustained-output
	printf 'FREMINAL_BIN must be built with --features frame-profiling,gpu-profiling: this validation requires the task 125.5/125.6 live-render-work summary and the task 125.8 and 125.9 GPU timing flush log lines.\n'
	local run_dir pid output
	run_dir=$(mktemp -d "${TMPDIR:-/tmp}/freminal-task125-profile-smoke.XXXXXX")
	launch_terminal freminal blink profile-smoke "${run_dir}"
	pid=${LAUNCHED_PID}
	task125_start_workload sustained-output "${SAMPLE_DURATION}" "${run_dir}" >/dev/null
	sleep "$((SAMPLE_WARMUP + SAMPLE_DURATION))"
	verify_grid "${run_dir}"
	task125_verify_process_gpu "${pid}"
	output=$(<"${run_dir}/freminal.stdout.log")
	[[ ${output} == *'Active OpenGL renderer: AMD Radeon RX 7900 XTX'* ]] || {
		printf 'profile smoke did not record the expected Navi 31 renderer\n' >&2
		return 1
	}
	[[ ${output} == *'live render-work profile (task 125.5/125.6)'* ]] || {
		printf 'profile smoke did not record a Task 125 live-render summary\n' >&2
		return 1
	}
	[[ ${output} == *'Task 125.8 terminal GPU timing flush'* ]] || {
		printf 'profile smoke did not record a Task 125.8 GPU timing flush\n' >&2
		return 1
	}
	[[ ${output} == *'Task 125.9 chrome and frame GPU timing flush'* ]] || {
		printf 'profile smoke did not record a Task 125.9 chrome and frame GPU timing flush\n' >&2
		return 1
	}
	printf 'Profile smoke validated: %s\n' "${run_dir}/freminal.stdout.log"
	task125_terminate_tree "${pid}" "${OWNED_START_TIMES[${pid}]}"
	if [[ ${LAUNCHED_SHELL_PID} != "${pid}" ]]; then
		task125_terminate_tree \
			"${LAUNCHED_SHELL_PID}" "${OWNED_START_TIMES[${LAUNCHED_SHELL_PID}]}"
	fi
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
	btop | sparse-row | sustained-output | streaming-output)
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
	verify_series_geometry "${run_dir}"
	prepare_workload_before_warmup "${workload}" "${run_dir}"
	sleep "${SAMPLE_WARMUP}"
	verify_grid "${run_dir}"
	if task125_verify_process_gpu "${pid}"; then
		printf 'available\n' >"${run_dir}/gpu-process-status"
	else
		printf 'unavailable\n' >"${run_dir}/gpu-process-status"
	fi
	start_workload_after_warmup "${workload}" "${run_dir}"
	capture_collectors "${pid}" "${SAMPLE_DURATION}" "${run_dir}"
	append_sample_csv "${csv}" "${repeat}" "${workload}" "${terminal}" "${run_dir}"
	task125_terminate_tree "${pid}" "${OWNED_START_TIMES[${pid}]}"
	if [[ ${LAUNCHED_SHELL_PID} != "${pid}" ]]; then
		task125_terminate_tree \
			"${LAUNCHED_SHELL_PID}" "${OWNED_START_TIMES[${LAUNCHED_SHELL_PID}]}"
	fi
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
	ensure_external_output "${output_dir}"
	mkdir -p "${output_dir}/raw"
	GEOMETRY_BASELINE_FILE="${output_dir}/${label}-hyprland-geometry.tsv"
	GEOMETRY_BASELINE_INITIALIZED=false
	csv="${output_dir}/${label}-samples.csv"
	printf '%s\n' 'repeat,workload,terminal,wall_seconds,task_clock_ms,user_task_clock_ms,kernel_task_clock_ms,cycles,instructions,context_switches,wakeups,gpu_gfx_ns,grid_rows,grid_cols' >"${csv}"

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
		streaming-output chrome-blink chrome-steady
}

run_pointer_screen() {
	local output_dir
	output_dir=$(matrix_output_dir "$@")
	SAMPLE_WARMUP=${SCREEN_WARMUP}
	SAMPLE_DURATION=${SCREEN_DURATION}
	SAMPLE_REPEATS=${SCREEN_REPEATS}
	run_series pointer-screen "${output_dir}" pointer
}

run_collector_smoke() {
	local output_dir
	output_dir=$(matrix_output_dir "$@")
	SAMPLE_WARMUP=5
	SAMPLE_DURATION=10
	SAMPLE_REPEATS=1
	run_series collector-smoke "${output_dir}" idle-blink
}

run_collector_smoke_one() {
	local terminal=$1
	shift
	local output_dir csv
	output_dir=$(matrix_output_dir "$@")
	ensure_external_output "${output_dir}"
	mkdir -p "${output_dir}/raw"
	GEOMETRY_BASELINE_FILE="${output_dir}/collector-smoke-one-hyprland-geometry.tsv"
	GEOMETRY_BASELINE_INITIALIZED=false
	csv="${output_dir}/collector-smoke-one-samples.csv"
	printf '%s\n' 'repeat,workload,terminal,wall_seconds,task_clock_ms,user_task_clock_ms,kernel_task_clock_ms,cycles,instructions,context_switches,wakeups,gpu_gfx_ns,grid_rows,grid_cols' >"${csv}"
	SAMPLE_WARMUP=5
	SAMPLE_DURATION=10
	SAMPLE_REPEATS=1
	collector_preflight
	capture_one "${output_dir}" 1 idle-blink "${terminal}" "${csv}"
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
	local command=$1 terminal
	shift
	parse_args "$@"
	case "${command}" in
	preflight)
		preflight
		;;
	collector-preflight)
		preflight
		collector_preflight
		;;
	smoke)
		(($# >= 2)) || {
			usage
			return 2
		}
		smoke "$1" "$2"
		;;
	profile-smoke)
		profile_smoke
		;;
	screen)
		run_screen "$@"
		;;
	pointer-screen)
		run_pointer_screen "$@"
		;;
	collector-smoke)
		run_collector_smoke "$@"
		;;
	collector-smoke-one)
		(($# >= 1)) || {
			usage
			return 2
		}
		terminal=$1
		shift
		run_collector_smoke_one "${terminal}" "$@"
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

if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
	main "$@"
fi
