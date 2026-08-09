---
name: freminal-state-representation
description: Use ONLY in the freminal repository when about to add a bool field or bool parameter, pass a bare true/false, or lean on an excessive_bools allow. Carries the buffer_states/modes/ exemplar, the rule that this skill overrides freminal-architecture's narrower "raw bool is OK when no enum exists" hatch for anything crossing the snapshot boundary, the accepted-boilerplate decision, and the freminal-specific compatibility stop conditions. The generic bool-to-enum rules live in the shared state-representation skill.
---

# Freminal: state is a named enum

The generic rules -- name the enum for its domain, the three cases
where a bool MUST become an enum (parameters, mutually-exclusive pairs,
values crossing a boundary), the three cases where a bool is correct
(independent simultaneous signals, modifier sets, external-format
toggles) -- live in the shared **`state-representation`** skill. Read
that first. This skill carries only the freminal-specific parts.

## The exemplar already in the repo

`freminal-common/src/buffer_states/modes/` -- 37 files, one named enum
each (`Decawm`, `Dectcem`, `Decckm`, `Decscnm`, ...). `agents.md`
already mandates using them over raw bools for terminal modes. **This
skill extends that same rule from terminal modes to all state.** Match
that pattern.

## This skill overrides the architecture skill's hatch

`freminal-architecture` says a raw `bool` is acceptable "only when no
enum exists". **That hatch is narrower than it reads, and this skill
takes precedence.**

If the value is transported -- through `TerminalSnapshot`, a
`crossbeam` channel, or any public signature -- create the enum rather
than taking the hatch. Every terminal mode reaching the GUI crosses the
snapshot boundary, so that is nearly all of them. A raw `bool` is fine
only for a value that never leaves the function computing it.

## Boilerplate is accepted, by decision

The real cost of an enum here is boilerplate (`Display`, `FromStr`,
serde impls). By maintainer decision **boilerplate and extra LOC are
explicitly accepted** in exchange for readability. Do not cite
verbosity, LOC, or layout as a reason to keep a bool. The measured
runtime cost in this codebase is zero.

## Existing suppressions are evidence, not permission

`#[allow(clippy::struct_excessive_bools)]` and
`#[allow(clippy::fn_params_excessive_bools)]` in the tree record a
previous decision to add one more bool. **Do not read one as licence to
add another.** If you are about to add a bool to a struct that already
carries the allow, that is precisely the trigger for this skill. Either
the struct is one of the legitimate cases -- in which case say so, and
the allow should carry a comment explaining which -- or your new field
wants an enum.

Where the new enum should *live* is a separate question:
`freminal-module-cohesion` (which file) and
`freminal-extend-or-extract` (does it belong here at all).

## When to stop and ask

- Converting a bool would change a serialised format -- a config key, a
  layout file, a `.frec` recording field. Stop; that is a compatibility
  question, not a style one. Config keys additionally go through
  `freminal-config-options`.
- The bool is one of the legitimate cases but the struct is genuinely
  unwieldy and `bitflags` would be a new dependency. Stop and ask.
- The right enum spans a crate boundary and would need a new type in
  `freminal-common`. Fine in principle, but say so -- it affects every
  downstream crate and recompiles the whole tree.
- You are tempted to introduce one generic two-state enum to fix many
  bools at once. Don't. One enum per concept.
