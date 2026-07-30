---
name: freminal-state-representation
description: Use ONLY when working in the freminal repository AND you are about to add a `bool` field to a struct, add a `bool` parameter to a function, add or rely on an existing `#[allow(clippy::struct_excessive_bools)]` or `#[allow(clippy::fn_params_excessive_bools)]`, pass a bare `true` / `false` at a call site, or introduce a pair of `bool`s that cannot legally both be true. Also triggers on "should this be an enum", "is_active / is_enabled / has_x flag", and on naming a two-state type. Codifies the named-domain-enum rule (`BlinkState::Enabled`, never a shared generic `Enabled` / `Disabled`), states the three cases where a bool MUST become an enum, and — equally important — the three cases where a bool is correct and an enum would be worse.
---

# Freminal: state is a named enum, not a bare bool

Agents reach for `bool` by default and freminal now carries **12
`excessive_bools` suppressions** across **19 structs with four or more
bool fields**. Some of those are correct (see "Where bools stay"), but
the default is wrong, and the suppressions have become permission
rather than a record of a decision.

**The measured cost of the fix is zero.** Measured with
`rustc 1.97.1 -O` on `x86_64-unknown-linux-gnu`:

```text
bool = 1 byte      fieldless enum = 1 byte
Option<bool> = 1   Option<enum>   = 1   (niche-packed)

struct { six bools }        = 6 bytes
struct { two enums }        = 2 bytes
```

Two honest caveats on those numbers, so nobody cites this skill as a
language guarantee:

- **Rust does not specify enum layout without an explicit `repr`.** The
  one-byte result is what rustc does for a fieldless enum on every
  target freminal supports, not something the language promises. If you
  need a guaranteed width — FFI, a serialised format, a wire protocol —
  say so with `#[repr(u8)]` rather than relying on the default.
- **`Option<enum>` is only niche-packed while the enum leaves a niche.**
  A two-variant enum leaves 254 spare values in a byte, so it packs. A
  fieldless enum with 256 variants leaves none, and `Option` of it
  grows.

**Neither caveat weakens the rule**, and that is the point: even if an
enum cost four bytes it would be irrelevant for a struct field or a
function parameter. Performance is not a reason to prefer `bool` here.
The real cost is boilerplate (`Display`, `FromStr`, serde), and by
maintainer decision **boilerplate and extra LOC are explicitly
accepted** in exchange for readability. Do not cite verbosity — or
layout — as a reason to keep a bool.

## The exemplar already in the repo

`freminal-common/src/buffer_states/modes/` — 37 files, one named enum
each (`Decawm`, `Dectcem`, `Decckm`, `Decscnm`, …). `agents.md` already
mandates using them over raw bools for terminal modes. **This skill
extends that same rule from terminal modes to all state.** Match that
pattern.

## Name the enum for its domain, never generically

```rust
// YES -- the type name says what this is about
enum BlinkState { Enabled, Disabled }
enum PaneFocus  { Active, Inactive }
enum EchoMode   { On, Off }

// NO -- a generic two-state enum reused across unrelated concepts
enum FreminalEnabled { Enabled, Disabled }   // shared by 20 callers
enum Toggle { On, Off }                      // says nothing
```

One enum per concept. A shared `Enabled` / `Disabled` type is a `bool`
with extra steps: it restores exactly the ambiguity the enum was
supposed to remove, because `Enabled` at a call site tells you nothing
about *what* is enabled. Reading `BlinkState::Enabled` should be
self-describing without looking up the parameter name.

Variant names should read naturally at the use site, and need not be
`Enabled` / `Disabled` — prefer the domain's own vocabulary
(`Active` / `Inactive`, `Visible` / `Hidden`, `Tracking` / `NoTracking`,
`Live` / `ScrolledBack`).

## When a bool MUST become a named enum

### 1. Bool parameters — always, no exceptions

This is the highest-value rule and has no counter-argument. A call site
like this is unreadable and unreviewable:

```rust
widget.show(ui, snap, ..., true, false, true, ...);
```

`freminal/src/gui/terminal/widget.rs` carries
`#[allow(clippy::fn_params_excessive_bools)]` on a 23-parameter
function for exactly this reason. If you are adding a bool parameter,
make it a named enum — or, if the signature is already wide, a named
params struct (see `freminal-extend-or-extract`).

### 2. Two or more bools that cannot legally both be true

This is a **correctness** rule, not a style preference: if the type
permits a state the program considers impossible, the type is wrong.
Collapse them into one enum so the illegal combination cannot be
constructed.

```rust
// NO -- (true, true) is meaningless but representable
struct S { is_live: bool, is_scrolled_back: bool }

// YES
enum ViewPosition { Live, ScrolledBack { rows: usize } }
```

If you find yourself writing a `debug_assert!` that two bools are not
both set, you have found this case.

### 3. Bools crossing a boundary

Any bool that crosses a **crate**, **thread**, **frame**, or **public
API** boundary becomes a named enum. Inside one function's body a local
`let is_first = true;` is fine; the moment the value is transported it
needs to be self-describing at the far end, where the local context
that made the bool obvious is gone.

#### Precedence over the terminal-mode escape hatch

`freminal-architecture` says of terminal modes: "Raw `bool` is OK only
when no enum exists." **This rule takes precedence over that hatch.**
Read together:

- Mode **has** an enum in `freminal-common/src/buffer_states/modes/` →
  use it. Never a raw `bool`. (Unchanged.)
- Mode has **no** enum yet, and the value is **transported** — through
  `TerminalSnapshot`, `SnapshotModeFields`, `InputEvent`, a channel, or
  any public signature → **create the enum.** Do not take the hatch.
  Every mode reaching the GUI crosses the snapshot boundary, so in
  practice this is nearly all of them.
- Mode has **no** enum and the value never leaves the function that
  computes it → a local `bool` is fine.

This is what Task 26 ("Bool-to-Enum Mode Refactor") did, and the hatch
is the unfinished half of it, not a standing exemption. Adding a new
transported mode as a raw `bool` because "no enum exists yet" is
circular: you are the one who would be creating it.

## Where bools stay (do NOT convert these)

An enum is worse in all three of these. Converting them is churn, and a
review will reject it.

### Independent simultaneous signals

A bag of unrelated yes/no observations where several are true at once is
correctly a set of bools. Examples in-tree:
`ChromeSignals` (15 bools, `chrome_damage.rs`),
`PointerMotionConditionFlags` (8, `window.rs`),
`DismissiblePresence` (7), `ChromeGatePredicates` (4),
`ChromeTabSnapshotDiff` (4). These are observation records, not states.
An enum cannot express "six of these fired". If the set grows unwieldy,
`bitflags` is the alternative — **not** an enum.

### Modifier / flag sets

`KeyModifiers` (8 bools), `RawKeyMods` (4). Simultaneous by definition.
Leave them.

### Config toggles deserialised from TOML

`NotificationsConfig` (6 bools), `PasteGuardConfig` (4). `enabled = true`
is idiomatic TOML and what users expect; an enum forces
`enabled = "Enabled"` and makes `config_example.toml` worse. The config
struct is a **serialisation boundary**, and the user-facing format wins.
Convert at the internal boundary instead if the value then travels — see
`freminal-config-options`.

## Existing suppressions are evidence, not permission

`#[allow(clippy::struct_excessive_bools)]` and
`#[allow(clippy::fn_params_excessive_bools)]` in the tree record a
previous decision to add one more bool. **Do not read one as licence to
add another.** If you are about to add a bool to a struct that already
carries the allow, that is precisely the trigger for this skill. Either
the struct is one of the legitimate cases above — in which case say so,
and the allow should carry a comment explaining which — or your new
field wants an enum.

Where the new enum should *live* is a separate question:
`freminal-module-cohesion` (one concept per module) and
`freminal-extend-or-extract` (does this belong here at all).

## When to stop and ask

- Converting a bool would change a serialised format — a config key, a
  layout file, a `.frec` recording field. Stop; that is a
  compatibility question, not a style one.
- The bool is one of the legitimate cases but the struct is genuinely
  unwieldy and `bitflags` would be a new dependency. Stop and ask.
- The right enum spans a crate boundary and would need a new type in
  `freminal-common`. That is fine in principle, but say so — it affects
  every downstream crate.
- You are tempted to introduce one generic two-state enum to fix many
  bools at once. Don't. That is the anti-pattern this skill names.
  One enum per concept.
