#!/usr/bin/env bash
# Measure the Instant File Search indexer's resident memory for the synthetic
# benchmark corpus (disk vs memory storage mode).
#
# Safe by construction:
#   * runs the read-only `benchmark` subcommand of the installed indexer;
#   * TMPDIR is redirected into a fresh `mktemp -d` directory, so the throwaway
#     SQLite database never touches /var/lib/instant-file-search;
#   * never starts/stops the systemd service and never writes the live index.
#
# Usage:
#   scripts/measure-indexer-memory.sh [ENTRY_COUNT]
#
#   ENTRY_COUNT defaults to 1600000 (this machine's real corpus is 1,608,934
#   entries: / + /boot/efi + /mnt/2tb-drive).
#
# Environment overrides:
#   BIN             indexer binary (default: installed release binary)
#   MODES           storage modes to measure (default: "disk memory").
#                   Use MODES="disk stream" to compare the collecting scan path
#                   with the streaming initial-scan path.
#   EXTRA_COUNTS    extra entry counts, space separated (default: "" )
#                   e.g. EXTRA_COUNTS="250000 500000"
#   SAMPLE_INTERVAL seconds between /proc samples (default: 0.2)
#
# Output: one compact table. Peak comes from the kernel's VmHWM high-water mark
# (monotonic, so a sample after the build captures the true peak); steady is the
# last VmRSS sampled before the process exited; rss_after_build is the value the
# benchmark itself reports in its JSON line.

set -euo pipefail

BIN="${BIN:-/usr/local/lib/instant-file-search/instant-file-search-indexer}"
ENTRY_COUNT="${1:-1600000}"
MODES="${MODES:-disk memory}"
EXTRA_COUNTS="${EXTRA_COUNTS:-}"
SAMPLE_INTERVAL="${SAMPLE_INTERVAL:-0.2}"

if [[ ! -x "$BIN" ]]; then
	echo "error: indexer binary not found or not executable: $BIN" >&2
	exit 1
fi
if [[ ! "$ENTRY_COUNT" =~ ^[0-9]+$ ]] || ((ENTRY_COUNT < 1)); then
	echo "error: ENTRY_COUNT must be a positive integer, got: $ENTRY_COUNT" >&2
	exit 1
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ifs-bench.XXXXXX")"
# Non-destructive by design: this script never deletes anything. It leaves its
# throwaway directory in place and prints the path so a human can remove it
# deliberately after inspecting the output.
cleanup() {
	echo "benchmark dir left in place: $WORK" >&2
}
trap cleanup EXIT

kb_to_mib() { awk -v kb="$1" 'BEGIN { printf "%.1f", kb / 1024 }'; }
to_mib() { awk -v b="$1" 'BEGIN { printf "%.1f", b / 1048576 }'; }

json_num() {
	# json_num <json> <key> -> first numeric value for key, empty if absent
	printf '%s' "$1" | sed -n "s/.*\"$2\":\([0-9][0-9.eE+-]*\).*/\1/p" | head -n 1
}

# run_one <mode> <entries> -> prints one "|"-separated row
run_one() {
	local mode="$1" count="$2"
	local rundir="$WORK/$mode-$count"
	mkdir -p "$rundir"
	local out="$rundir/out.json" err="$rundir/err.log"

	local started now peak_hwm_kb=0 peak_rss_kb=0 steady_rss_kb=0
	local rss hwm state
	started="$(date +%s.%N)"
	(
		cd "$rundir" && TMPDIR="$rundir" exec "$BIN" benchmark "$mode" "$count"
	) >"$out" 2>"$err" &
	local pid=$!

	# Stop sampling once the child is gone or a zombie (an unreaped child keeps
	# its /proc entry, so `kill -0` alone can spin forever).
	while [[ -r "/proc/$pid/status" ]]; do
		state="$(awk '/^State:/{print $2}' "/proc/$pid/status" 2>/dev/null || true)"
		[[ "$state" == "Z" ]] && break
		rss="$(awk '/^VmRSS:/{print $2}' "/proc/$pid/status" 2>/dev/null || true)"
		hwm="$(awk '/^VmHWM:/{print $2}' "/proc/$pid/status" 2>/dev/null || true)"
		if [[ "$rss" =~ ^[0-9]+$ ]]; then
			steady_rss_kb="$rss"
			if ((rss > peak_rss_kb)); then peak_rss_kb="$rss"; fi
		fi
		if [[ "$hwm" =~ ^[0-9]+$ ]]; then
			if ((hwm > peak_hwm_kb)); then peak_hwm_kb="$hwm"; fi
		fi
		sleep "$SAMPLE_INTERVAL"
	done

	local rc=0
	wait "$pid" || rc=$?
	now="$(date +%s.%N)"
	local wall
	wall="$(awk -v a="$started" -v b="$now" 'BEGIN { printf "%.1f", b - a }')"

	if ((rc != 0)); then
		echo "benchmark $mode $count FAILED (exit $rc):" >&2
		sed -n '1,20p' "$err" >&2
		return 1
	fi

	local json rss_after db_bytes build_ms
	json="$(tail -n 1 "$out")"
	rss_after="$(json_num "$json" rss_bytes_after_build)"
	db_bytes="$(json_num "$json" database_bytes)"
	build_ms="$(json_num "$json" build_ms)"
	[[ -n "$rss_after" ]] || rss_after=0
	[[ -n "$db_bytes" ]] || db_bytes=0
	[[ -n "$build_ms" ]] || build_ms=0

	local peak_kb="$peak_hwm_kb"
	if ((peak_rss_kb > peak_kb)); then peak_kb="$peak_rss_kb"; fi

	printf '| %-6s | %9s | %10s | %10s | %10s | %8s | %12s | %10s |\n' \
		"$mode" "$count" "$(kb_to_mib "$peak_kb")" "$(kb_to_mib "$steady_rss_kb")" \
		"$(to_mib "$rss_after")" "$wall" "$(to_mib "$db_bytes")" "$(awk -v m="$build_ms" 'BEGIN{printf "%.0f", m}')"
}

printf 'indexer: %s (%s)\n' "$BIN" "$("$BIN" --version 2>/dev/null || echo version-unknown)"
printf 'machine: %s  sampled at %s  interval=%ss\n' \
	"$(uname -srm)" "$(date -Is)" "$SAMPLE_INTERVAL"
printf '| %-6s | %9s | %10s | %10s | %10s | %8s | %12s | %10s |\n' \
	"mode" "entries" "peak_MiB" "steady_MiB" "rss_after_MiB" "wall_s" "database_MiB" "build_ms"
printf '|%s|%s|%s|%s|%s|%s|%s|%s|\n' \
	"--------" "-----------" "------------" "------------" "------------" "----------" "--------------" "------------"

run_one_row_counts() {
	local count="$1" mode
	for mode in $MODES; do
		run_one "$mode" "$count"
	done
}

run_one_row_counts "$ENTRY_COUNT"
for extra in $EXTRA_COUNTS; do
	if [[ ! "$extra" =~ ^[0-9]+$ ]] || ((extra < 1)); then
		echo "error: EXTRA_COUNTS entry must be a positive integer, got: $extra" >&2
		exit 1
	fi
	run_one_row_counts "$extra"
done

echo
echo "note: peak_MiB is VmHWM (kernel high-water mark), steady_MiB is the last VmRSS"
echo "      sample before exit, rss_after_MiB is the benchmark's own"
echo "      rss_bytes_after_build (measured after the build input is dropped)."
echo "note: benchmark disk mode still materialises synthetic_entries() in RAM;"
echo "      compare disk vs memory rows with that in mind."
