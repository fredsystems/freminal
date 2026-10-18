# Fuzzing Freminal

This directory is a standalone [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz)
crate (`freminal-fuzz`). It feeds hostile input to the parts of Freminal that
parse untrusted bytes: escape sequences, recordings, layouts, config files,
image formats and the scrollback row format.

- It is **excluded from the root workspace** (`exclude = ["fuzz"]` in the root
  `Cargo.toml`) and has its own `[workspace]` table and its own `Cargo.lock`
  (tracked). Nightly compiler flags and sanitizer instrumentation therefore can
  never leak into a normal `cargo build` / `cargo xtask ci`.
- It needs a **nightly toolchain** (`-Zsanitizer=address`, libFuzzer
  instrumentation). The toolchain, `cargo-fuzz` and (on Linux) `bubblewrap` live
  in a dedicated dev shell: `nix develop .#fuzz`. The shell is separate from
  `default` and `ci` on purpose: fuzz tooling does not belong in the everyday
  shell, and `cargo` there must never resolve to a nightly compiler. The fuzz
  shell contains the nightly as the **only** `cargo`, because cargo-fuzz runs
  whichever `cargo` is first on `PATH`.
- Everything is driven through `cargo xtask fuzz*` (see
  [Running locally](#running-locally)). `cargo xtask fuzz-list` works on stable;
  the other subcommands need the fuzz shell.

## Targets

Run `cargo xtask fuzz-list` for the authoritative list. "No invariant" means the
only property checked is that the code under test does not panic, hang or run
out of memory (AddressSanitizer also reports memory errors in any `unsafe` or
FFI code reached).

| Target                 | Covers                                                                                                                                                | Invariants asserted                                                                                                                                                                                 |
| ---------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ansi_parser`          | `FreminalAnsiParser::push` alone, one parser instance across several chunks (split CSI/OSC/DCS/APC and UTF-8 sequences); every output is `Display`ed. | No invariant.                                                                                                                                                                                       |
| `terminal_state`       | The whole emulator headlessly (`TerminalEmulator::new_headless`): parser, handler, buffer, image store, reply generation, periodic `build_snapshot`.  | No invariant. **Sandbox mandatory**, see [Why a sandbox](#why-a-sandbox).                                                                                                                           |
| `frec_parse`           | `parse_recording_from_bytes`: FREC v2 header, MessagePack metadata, event stream, seek index, footer.                                                 | No invariant.                                                                                                                                                                                       |
| `kitty_graphics_parse` | `parse_kitty_graphics`: APC envelope, control data, `key=value` keys, base64 payload. Parsing only, never touches the filesystem.                     | No invariant.                                                                                                                                                                                       |
| `osc99_parse`          | `parse_osc_99` (kitty notifications); input is split at the first `;` into metadata and payload.                                                      | No invariant.                                                                                                                                                                                       |
| `sixel_parse`          | `parse_sixel` on DCS inner content; attacker-controlled raster size, repeat counts and colour registers (the likeliest runaway allocation).           | No invariant.                                                                                                                                                                                       |
| `compact_row`          | `CompactRow::from_bytes` (the scrollback compression format), then `to_row` and re-encode.                                                            | Consumed length never exceeds the input; `from_bytes(row.to_bytes())` succeeds and consumes every byte; re-encoding is stable (compared on the encoding).                                           |
| `base64`               | `decode`, `decode_strict` and `StreamDecoder` on raw input, plus encode/decode round trips.                                                           | Strict success implies lenient agrees; padded and unpadded round trips return the data; `StreamDecoder` equals one-shot `decode` on encoder output (not asserted for raw input, where they differ). |
| `layout_toml`          | `Layout::from_str_content`, then `validate`, `resolve` and `to_toml_string`. Deliberately skips `apply_variables` and file I/O.                       | No invariant.                                                                                                                                                                                       |
| `config_toml`          | Deserialising `Config` from TOML (the `Config` derive and the `toml` parser only; `ConfigPartial`, `apply_partial` and `validate` are private).       | No invariant.                                                                                                                                                                                       |

Targets that take chunked input (`ansi_parser`, `terminal_state`, `base64`)
derive their chunk lengths from the first input byte(s) via
`freminal_fuzz::split_chunks` (`src/lib.rs`), so a crashing input reproduces
exactly. Harness code in this crate is linted with the same clippy bar as
production code (see `[lints.clippy]` in `Cargo.toml`): it must not panic on
its own, only the code under test may.

## Directory layout and corpus policy

```text
fuzz/
  Cargo.toml, Cargo.lock   tracked
  fuzz_targets/<t>.rs      one harness per target
  src/lib.rs               helpers shared by more than one target
  seeds/<t>/               curated, small, COMMITTED, never written by any run
  regressions/<t>/         minimized crash reproducers, COMMITTED with the fix
  corpus/<t>/              local working corpus        (gitignored; a checkout of the
                           private corpus repository, see "Persistent corpus")
  artifacts/<t>/           crashes, OOMs, timeouts     (gitignored)
  target/                  build output                (gitignored)
```

- **`seeds/<t>`** is hand-picked input that gets libFuzzer into interesting
  states quickly. Keep it small. A run reads it but writes new inputs to the
  working corpus (the first directory on the libFuzzer command line), never
  here.
- **`regressions/<t>`** holds minimized reproducers of bugs that have been
  **fixed**. Commit a reproducer here in the same PR as its fix. Do **not**
  commit a reproducer for a bug that still crashes: `fuzz-replay` replays this
  directory and is a required CI gate, so a live reproducer would block every PR.
  Attach an unfixed reproducer to the GitHub issue instead.
- **`corpus/`** and **`artifacts/`** are local working directories and are
  gitignored. The corpus is a **checkout of the private repository
  `fredsystems/freminal-fuzz-corpus`** (layout `<target>/<sha1>`), kept in sync
  with `cargo xtask fuzz-corpus pull` / `push` (see
  [Persistent corpus](#persistent-corpus)). It is never committed to this
  repository. **Seeds and regressions stay here**, in `seeds/` and
  `regressions/`: they are curated and reviewed, the corpus is machine-grown.
- Seeds and regressions are **byte-exact**: CR/LF, NUL, trailing whitespace and
  binary encodings are the point. They are excluded from the pre-commit text
  fixers (`extraExcludes` in `flake.nix`, which generates `.pre-commit-config.yaml`) and marked
  `binary` in `.gitattributes` so git never normalises them. Do not open and
  re-save them in an editor.

## Running locally

All commands run from the repository root. Enter the fuzz shell first, or prefix
each command with `nix develop .#fuzz --command`.

```bash
nix develop .#fuzz

cargo xtask fuzz-list                       # list targets (works on stable)
cargo xtask fuzz ansi_parser                # 300 s (the default), stop at first finding
cargo xtask fuzz osc99_parse --time 120     # 2 minutes
cargo xtask fuzz sixel_parse --time 600 --on-crash keep-going --jobs 4
cargo xtask fuzz-smoke --time 45            # every target, 45 s each (the default)
cargo xtask fuzz-replay                     # replay seeds + regressions, no fuzzing
cargo xtask fuzz-cmin ansi_parser           # minimise one working corpus
cargo xtask fuzz-cmin --all                 # minimise every working corpus
cargo xtask fuzz-corpus pull                # fetch the persistent corpus
cargo xtask fuzz-corpus push                # minimise, commit and publish it
```

Short aliases exist: `fz`, `fs`, `fr`, `fcm`, `fl`.

| Subcommand               | What it does                                                                                                                                               |
| ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `fuzz <t> [--time SECS]` | Builds the target and fuzzes it for `--time` seconds (default 300). `--on-crash stop` (default) ends at the first finding.                                 |
| `--on-crash keep-going`  | Fork mode (`-fork=<jobs>`, ignoring crashes, OOMs and timeouts): collects every finding as an artifact. `--jobs N` sets the workers; stop mode ignores it. |
| `fuzz-smoke`             | Builds and fuzzes every target briefly in stop mode, prints a summary table, exits non-zero if any target failed.                                          |
| `fuzz-replay`            | Lints and tests this crate, then runs every file in `seeds/<t>` and `regressions/<t>` once through its target, naming any file that fails. The CI gate.    |
| `fuzz-cmin <t>`          | libFuzzer merge: rewrites `<corpus-root>/<t>` as a minimal corpus with the same coverage.                                                                  |
| `fuzz-cmin --all`        | The same for every target; targets with no working corpus are skipped.                                                                                     |
| `fuzz-corpus pull\|push` | Syncs the working corpus with the private corpus repository; see [Persistent corpus](#persistent-corpus).                                                  |

`fuzz-cmin` replaces the corpus by renaming, so an interrupted run is
recoverable. It merges `<corpus-root>/<t>` into `<t>.cmin`, parks the original
as `<t>.old`, renames `.cmin` into place and then deletes `.old`. On the next
run: if `<t>` is missing but `<t>.old` exists, the original is restored
automatically before minimising; if both exist, `xtask` stops and deletes
nothing (it cannot know which is the good corpus), so keep the one you want as
`<t>` and remove the other. A merge that produces no files from a non-empty
corpus is refused and leaves `<t>` untouched. If only the final deletion of
`.old` fails, the minimised corpus is already installed: `xtask` logs a warning
naming the leftover `.old` directory and succeeds, but the next `fuzz-cmin`
stops at the both-exist check until you delete it by hand.

Every run passes libFuzzer limits of 2048 MB RSS, 2048 MB per allocation and 25 s
per input.

The sandbox and directories are also available as flags on every command that
runs a fuzz binary: `--sandbox bwrap|none`, `--corpus-root DIR` and
`--artifacts-root DIR`. The two directories can be set through environment
variables:

| Variable                      | Default          | Meaning                                             |
| ----------------------------- | ---------------- | --------------------------------------------------- |
| `FREMINAL_FUZZ_CORPUS_DIR`    | `fuzz/corpus`    | Root of the working corpora (`<DIR>/<target>/`).    |
| `FREMINAL_FUZZ_ARTIFACTS_DIR` | `fuzz/artifacts` | Root receiving crash artifacts (`<DIR>/<target>/`). |

Relative paths resolve against the workspace root.

**Suggested cadence:** when you change parser, buffer or other code a target
covers, run that target for one to five minutes (`--time 60` to `--time 300`)
and run `cargo xtask fuzz-replay` before pushing.

### Persistent corpus

Coverage accumulates over days, so the working corpus is persisted in a
separate **private** git repository, `fredsystems/freminal-fuzz-corpus`, laid
out as `<target>/<sha1>` at its root. The corpus root (`fuzz/corpus`, or
`FREMINAL_FUZZ_CORPUS_DIR`) **is** the working tree of a clone of it.

Long local run:

```bash
cargo xtask fuzz-corpus pull     # clone (first time) or update the corpus
# ... fuzz as long as you like ...
cargo xtask fuzz-corpus push     # minimise, commit, publish
```

- **`pull`** clones the repository into the corpus root, or updates an existing
  checkout. Local corpus files that are not yet in the repository are first
  set aside in a local commit, then rebased onto the remote. It refuses to touch
  a root that is a checkout of some other repository, or a non-empty directory
  that holds anything other than target directories.
- **`push`** pulls again, runs `fuzz-cmin` on every target, commits whatever
  changed (`corpus: <host> <timestamp>`, with a per-target file count in the
  body) and pushes. If the remote moved in the meantime it fetches, rebases and
  pushes again, up to three pushes in all. It fails, publishing nothing, if the
  minimisation fails or if the root is not a checkout (run `pull` first).
  Minimisation also **deletes** corpus files whose coverage is redundant; that
  deletion is published too.
- **Merging is conflict-free.** libFuzzer names each corpus file by the SHA-1 of
  its content, so one name always means the same bytes. Two machines that find
  the same input add the same file, and a rebase can never hit a content
  conflict.
- **History can be squashed.** Only the latest tree matters; the repository's
  history exists to make concurrent pushes mergeable, and may be squashed and
  force-pushed by the maintainer whenever it grows too large. A checkout made
  before a squash needs a fresh clone (delete the corpus root and `pull`).
- **Options.** `--repo-url` / `FREMINAL_FUZZ_CORPUS_REPO` (default
  `https://github.com/fredsystems/freminal-fuzz-corpus.git`), `--corpus-root` /
  `FREMINAL_FUZZ_CORPUS_DIR`, `--artifacts-root` and `--sandbox` (applies to the
  minimisation `push` runs).
- **Authentication.** Locally, git uses your normal credentials for github.com
  (credential manager, `gh auth setup-git`, ssh via `--repo-url`, ...) and may
  prompt. In CI, the `PAT` repository secret is passed to the pull and push
  steps as `FREMINAL_FUZZ_CORPUS_TOKEN`; `xtask` hands it to git through an
  inline credential helper that reads it from the environment, so it never
  appears in a command line or on disk, and git never prompts.
- **Adopting an existing local corpus.** If `fuzz/corpus` already holds a corpus
  from before the repository existed (only target directories, plus `.cmin` /
  `.old` scratch directories), `pull` turns it into a checkout in place: files
  that are already in the repository are identical by name, and everything else
  stays untracked until `push` commits it.
- **Why private.** Corpus inputs are by construction close to the inputs that
  crash the code, including crashes behind advisories that are still open.

Recipe for a long local run over every target:

```bash
cargo xtask fuzz-corpus pull
for t in $(cargo xtask fuzz-list); do
  cargo xtask fuzz "$t" --on-crash keep-going --jobs 4 --time 1200
done
cargo xtask fuzz-corpus push
```

That is 20 minutes per target; findings land in `fuzz/artifacts/<t>/`.

### Why a sandbox

`terminal_state` drives the real terminal handler end to end, including kitty
graphics `t=f` / `t=t` / `t=s` transmission. Those paths **read arbitrary
paths** and **delete files that are marked as kitty temporaries** under `/tmp`
and `/dev/shm`. A fuzzer will happily generate such a payload, so an unsandboxed
local run can delete real files.

By default (`--sandbox bwrap`, Linux only) every fuzz process runs under
bubblewrap: a read-only root, a fresh `/dev` and `/proc`, **private empty tmpfs
mounts over `/tmp` and `/dev/shm`**, all namespaces unshared (so no network),
and only the corpus and artifact directories bound read-write. Consequences:

- **Read-only inputs must not live under `/tmp` or `/dev/shm`**: the sandbox
  replaces both with empty tmpfs mounts, so a binary, seed directory or corpus
  being minimised there would be invisible inside it, and `xtask` refuses
  them. The writable corpus and artifact directories may live anywhere: they
  are bound into the sandbox after the tmpfs mounts, so they stay visible.
- The build step is never sandboxed (it needs the network and writes
  `fuzz/target`); only the fuzz binary is.
- `--sandbox none` runs on the host. Use it **only in disposable environments**
  (CI VMs, throwaway containers). Never on a workstation with data you care
  about. The bwrap sandbox is Linux-only, so on other platforms you must pass
  `--sandbox none` and accept that risk knowingly.

### Exit codes: what counts as a failure

`xtask` classifies the fuzz process exit code:

| Exit code                  | Meaning                                                      | Stop mode (default)   | `keep-going` mode     |
| -------------------------- | ------------------------------------------------------------ | --------------------- | --------------------- |
| `0`                        | Clean                                                        | pass                  | pass                  |
| `77`                       | Crash (libFuzzer `-error_exitcode`; ASan is mapped here too) | fail                  | pass with a warning   |
| `70`                       | Timeout (`-timeout_exitcode`)                                | fail                  | pass with a warning   |
| `71`                       | Out of memory (RSS or malloc limit)                          | fail                  | pass with a warning   |
| `72`                       | Interrupted (SIGINT)                                         | fail (infrastructure) | fail (infrastructure) |
| other / killed by a signal | Usage error or infrastructure problem                        | fail (infrastructure) | fail (infrastructure) |

In fork mode libFuzzer's parent exits with the **last** child's exit code, so a
finding code means "the run completed and the last worker hit a finding", not
"the run broke". Equally, a clean `0` does not mean nothing was found: the last
worker usually exits cleanly after the time budget stops it. A keep-going run
therefore never trusts the exit code for findings: it counts the new
`crash-`/`oom-`/`timeout-`/`leak-`/`slow-unit-` artifacts and always prints how
many there are and where. Artifact names are content hashes, so re-finding a
known input writes no new file; when nothing is new but the directory already
holds finding files, the summary says how many. It fails only when the run
itself did not complete.
AddressSanitizer would exit `1` by default, which collides with libFuzzer usage
errors, so `xtask` appends `exitcode=77` to `ASAN_OPTIONS` (after any value you
already set) so that every sanitizer finding reports as `77`.

## Triaging a finding

1. **Locate the input.** A stop run prints where to look: crashes land in
   `fuzz/artifacts/<t>/` (or `$FREMINAL_FUZZ_ARTIFACTS_DIR/<t>/`) as
   `crash-*`, `oom-*` or `timeout-*`. For nightly findings, download the
   `fuzz-nightly-findings` artifact from the workflow run.
2. **Reproduce** by running the built binary on the file. Replace
   `<host-triple>` with the output of `rustc -vV | sed -n 's/^host: //p'`
   (for example `x86_64-unknown-linux-gnu`):

   ```bash
   fuzz/target/<host-triple>/release/<t> fuzz/artifacts/<t>/crash-<hash>
   ```

   For `terminal_state` (and when in doubt, for any target) reproduce under the
   same sandbox `xtask` uses. The input file must not live under `/tmp` or
   `/dev/shm` (the sandbox hides both):

   ```bash
   bwrap --ro-bind / / --dev /dev --proc /proc \
     --tmpfs /tmp --tmpfs /dev/shm \
     --setenv TMPDIR /tmp \
     --unshare-all --die-with-parent --new-session -- \
     "$PWD/fuzz/target/<host-triple>/release/terminal_state" \
     "$PWD/fuzz/artifacts/terminal_state/crash-<hash>"
   ```

   Set `RUST_BACKTRACE=1` for a Rust panic backtrace.

3. **Minimize.** Inside `fuzz/`, in the fuzz shell:

   ```bash
   cd fuzz
   cargo fuzz tmin <t> artifacts/<t>/crash-<hash>
   ```

   `cargo fuzz tmin` runs the target **unsandboxed**, so use it directly only
   for targets other than `terminal_state`. For `terminal_state`, run libFuzzer's
   minimiser inside the same `bwrap` command as above, binding a writable output
   directory (outside `/tmp`) with an extra `--bind <dir> <dir>` placed before
   `--unshare-all`, and pass
   `-minimize_crash=1 -runs=10000 -exact_artifact_path=<dir>/minimized` after
   the binary.

4. **File a GitHub issue** with the minimized reproducer attached, the target
   name and the sanitizer or panic output. Do not commit the reproducer yet if
   the bug is still open (see the corpus policy above).
5. **Fix PR.** The PR that fixes the bug adds, in one commit series:
   - the minimized reproducer as `fuzz/regressions/<t>/<descriptive-name>`, so
     `fuzz-replay` guards against the bug returning; and
   - a unit or regression test in the **owning crate** (`freminal-buffer`,
     `freminal-common`, `freminal-terminal-emulator`, ...). The fuzz replay is a
     safety net, not a substitute for a focused test next to the code.

## CI

| Job / workflow   | Where                                | Gate                                                                                                                                                             |
| ---------------- | ------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `fuzz-replay`    | `.github/workflows/ci.yml`           | **Required** (part of `CI Success`). `cargo xtask fuzz-replay --sandbox none` (lints and tests this crate first). Deterministic: a failure is a real regression. |
| `fuzz-smoke`     | `.github/workflows/ci.yml`           | **Advisory** (`continue-on-error`). `cargo xtask fuzz-smoke --time 30 --sandbox none`; results go to the step summary and the `fuzz-smoke-artifacts` artifact.   |
| `Fuzz (nightly)` | `.github/workflows/fuzz-nightly.yml` | Not a PR check. Self-hosted, keep-going; `fuzz-corpus pull` first, `fuzz-corpus push` (minimise, commit, publish) last.                                          |

CI passes `--sandbox none` because bubblewrap cannot create user namespaces on
the hosted runners, which are disposable VMs anyway.

### Nightly run

`fuzz-nightly.yml` runs on `schedule` (02:00 UTC) and `workflow_dispatch` (inputs:
`minutes_per_target`, default 20; `jobs`, default 8). It must **never** be
triggered by `pull_request` or `pull_request_target`: the repository is public
and the job runs on a self-hosted machine, so only maintainer-controlled code may
reach it.

Runner contract:

- labels `self-hosted`, `Linux`, `freminal-fuzz`;
- `nix` (flakes enabled) on `PATH`;
- registered **non-ephemeral** (recommended): the work directory, including
  `fuzz/target`, then survives between jobs and saves a cold build. This is
  only a build-cache optimisation; nothing breaks if the work directory is
  wiped.

The runner needs **no persistent corpus directory**. The job sets
`FREMINAL_FUZZ_CORPUS_DIR` to a directory under the runner's temp dir, runs
`fuzz-corpus pull` (a failed pull fails the job before any fuzzing), fuzzes, and
then runs `fuzz-corpus push` (`if: always()` after a successful pull, so progress
is published even when a target had an infrastructure failure, but never after a
failed pull). Both steps authenticate with the `PAT` repository secret as
`FREMINAL_FUZZ_CORPUS_TOKEN`, set on those two steps only. Secrets are
available because the workflow runs only on `schedule` and `workflow_dispatch`.
`PAT` needs read/write access to the corpus repository.

Each target runs with `--on-crash keep-going --sandbox none` (the hardened
systemd unit of the runner is the sandbox). Findings are listed in the run
summary, uploaded as the `fuzz-nightly-findings` artifact (30 days), and
reported in a single tracking issue titled **"Fuzzing: findings from nightly
run"** (an existing open issue gets a comment, otherwise one is created).
Infrastructure failures (build errors, missing tools, interrupted runs) fail the
job without aborting the remaining targets.

## Adding a new target

1. Create `fuzz_targets/<t>.rs` starting with `#![no_main]` and a `//!` doc
   comment that states what the target covers, what it deliberately does **not**
   call, and the invariants asserted (or "none beyond does not panic, hang or run
   out of memory"). If the target can touch the filesystem or process
   environment, say so and make sure the sandbox covers it.
2. Add a `[[bin]]` entry to `Cargo.toml` (`name`, `path`, and `test = false`,
   `doc = false`, `bench = false` like the existing ones). `xtask` discovers
   targets from these entries.
3. Create `seeds/<t>/` with a few small, meaningful inputs (`xtask` requires the
   directory to exist).
4. Create `regressions/<t>/` containing an empty `.gitkeep`.
5. Add a row to the [Targets](#targets) table above.
6. Run `cargo xtask fuzz-list`, `cargo xtask fuzz <t> --time 60` and
   `cargo xtask fuzz-replay`.

## Bumping the nightly toolchain

The nightly is pinned by date so `nix flake update` cannot silently move it.
Edit `fuzzNightlyDate` in `flake.nix`. The date must exist in the locked
`rust-overlay` revision (`manifests/nightly/<year>/<date>.nix`); if it does not,
evaluation fails with a missing-attribute error, and you need to update the
`rust-overlay` input first. Afterwards run `cargo xtask fuzz-replay` in the new
shell.
