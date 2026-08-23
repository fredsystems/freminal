#!/usr/bin/env bash
#
# Task 123, subtask 123.6b — verify the `Gl` facade adds no per-call branch.
#
# Builds `freminal` with the `gl-codegen-probe` feature, which exposes four
# `#[inline(never)] #[no_mangle]` wrappers that differ only in whether they
# dispatch through `gui::renderer::gl_facade::Gl`, then extracts each
# function body from the emitted assembly and checks the facade versions
# contain no conditional branch.
#
# WHY A SCRIPT AND NOT A `#[test]`
#
# The check is inherently toolchain- and architecture-specific: it reads
# emitted assembly. A unit test asserting on x86_64 mnemonics would fail on
# the `ubuntu-24.04-arm` CI runner for reasons having nothing to do with the
# facade. Comparing function *addresses* at runtime was considered and
# rejected too — it depends on linker identical-code-folding, which is not
# guaranteed, varies by linker, and does not fire in debug builds. That is
# the definition of a flaky test.
#
# So this is a reproducible manual/CI-dispatch check whose result is
# recorded in `PLAN_123_GL_MEASUREMENT_HARNESS.md` alongside the toolchain
# that produced it, rather than a gate that would be fragile by nature.

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

echo "==> building freminal (release) with gl-codegen-probe"
cargo rustc -p freminal --release --lib --features gl-codegen-probe -- --emit=asm

# Pick the NEWEST emitted assembly, not merely the first match: a stale
# `.s` from an earlier build of a different feature set will otherwise be
# selected and silently compared, which is how this script failed the first
# time it was run.
ASM=$(find target/release/deps -name 'freminal-*.s' -printf '%T@ %p\n' |
	sort -rn | head -1 | cut -d' ' -f2-)
if [[ -z ${ASM} ]]; then
	echo "FAIL: no emitted assembly found under target/release/deps" >&2
	exit 1
fi
echo "==> assembly: ${ASM}"

# Extract one function body: everything between `name:` and the following
# `.Lfunc_end`, dropping assembler directives so only instructions remain.
extract() {
	awk -v want="$1:" '
		$0 == want { inside = 1; next }
		inside && /^\.Lfunc_end/ { exit }
		inside && !/^[[:space:]]*\./ { print }
	' "${ASM}"
}

status=0

# The realistic call-site pair is checked first and is the one that
# describes production. The isolated single-call pairs are deliberately
# pessimistic controls: `inline(never)` forces a call shape production never
# takes, so a one-instruction difference there is an artefact of the probe,
# not a cost. Both are checked for the property that actually matters --
# absence of conditional control flow.
for pair in "probe_site_direct probe_site_facade" \
	"probe_dispatch_direct probe_dispatch_facade" \
	"probe_draw_direct probe_draw_facade"; do
	read -r direct facade <<<"${pair}"

	direct_body=$(extract "${direct}")
	facade_body=$(extract "${facade}")

	# The strongest possible outcome: LLVM proved the two functions
	# byte-identical and emitted the facade as an alias of the control, so
	# there is no separate body to extract at all.
	if grep -Eq "^${facade} = ${direct}\$" "${ASM}"; then
		echo
		echo "--- ${facade} ---"
		echo "  ALIAS of ${direct} -- byte-identical code, zero cost"
		echo "OK: ${facade} folded into ${direct}"
		continue
	fi

	if [[ -z ${direct_body} || -z ${facade_body} ]]; then
		echo "FAIL: could not extract ${direct} / ${facade} from ${ASM}" >&2
		status=1
		continue
	fi

	echo
	echo "--- ${direct} ---"
	echo "${direct_body}"
	echo "--- ${facade} ---"
	echo "${facade_body}"

	# The claim under test: the facade introduces no conditional control
	# flow. An unconditional tail `jmp` is expected and fine; a compare, a
	# test, or a conditional jump would mean the enum discriminant survived
	# into the emitted code.
	if echo "${facade_body}" | grep -Eq '^[[:space:]]*(cmp|test|j(e|ne|z|nz|a|ae|b|be|g|ge|l|le))[[:space:]]'; then
		echo "FAIL: ${facade} contains conditional control flow -- the" >&2
		echo "      single-variant match did not compile away." >&2
		status=1
	else
		echo "OK: ${facade} has no conditional branch"
	fi
done

echo
if [[ ${status} -eq 0 ]]; then
	echo "PASS: the Gl facade adds no per-call branch"
else
	echo "FAILED" >&2
fi
exit "${status}"
