---
name: freminal-config-options
description: Use ONLY when working in the freminal repository AND adding, renaming, or removing a configuration option (a field on `Config` or any of its section structs in `freminal-common/src/config.rs`, e.g. a new `[notifications]`/`[command_blocks]` key or a whole new section). Codifies the mandatory wiring checklist that prevents the "user sets it in config.toml but it silently does nothing" bug class — the `ConfigPartial` / `apply_partial` omission that dropped `[notifications] enabled = true` (fix 76.4a).
---

# Freminal: adding a config option is a multi-file ritual

Freminal's config has a **layered-merge load path** that is easy to half-wire.
The canonical failure: you add a field to `Config`, set it in `config.toml`,
and it silently has no effect because the loader never copied your value onto
the defaults. This happened to `[notifications]`, `[command_blocks]`,
`[shell_integration]`, and `[tab_title]` — all added to `Config` but not to
`ConfigPartial` (fix `76.4a` / audit `76.4c`).

Read this before touching `freminal-common/src/config.rs`.

## Why it breaks: the load path

`load_config` does **not** deserialize `Config` directly. It:

1. Starts from `Config::default()`.
2. For each layer (system → user → `FREMINAL_CONFIG` env, or a single
   `--config` file), deserializes the TOML into a **separate** `ConfigPartial`
   struct whose fields are all `Option<…>`.
3. Merges each partial onto the running `Config` via `Config::apply_partial`,
   which copies a section **only if** `partial.<section>.is_some()`.

So a `Config` field that is missing from `ConfigPartial`, or has no arm in
`apply_partial`, is **never populated from user TOML**. It compiles fine and
fails silently at runtime. `save_config`, by contrast, serializes the whole
`Config` — so the round trip is asymmetric and the bug is invisible to a
naive "does it serialize?" check.

## The checklist — every step is mandatory

When adding a **new field to an existing section struct** (e.g. a new key in
`NotificationsConfig`):

1. **Add the field** to the section struct, with a doc comment.
2. **Default it** in that struct's `Default` impl (the struct has
   `#[serde(default)]`, so also fine to rely on `#[serde(default)]` per-field,
   but the `Default` impl must produce the intended default).
3. **Document it** in `config_example.toml` under the right `[section]`, with
   the default value shown (commented out).
4. **Surface it in the Settings UI** if user-facing: a widget in
   `freminal/src/gui/settings.rs` and, if it triggers an action, a dispatch
   arm in `freminal/src/gui/settings_dispatch.rs`.
5. **Mirror it in the Nix home-manager module**
   (`nix/home-manager-module.nix`) as an `mkOption`, so managed installs can
   set it. (See "The Nix home-manager module mirror" below for the three
   edits this requires.)
6. **Test it**: a round-trip test (`toml::to_string_pretty` → `toml::from_str`
   → assert the field survived) in the section's test group.

No `ConfigPartial` / `apply_partial` change is needed here, because the whole
section sub-struct deserializes as a unit.

When adding a **whole new section** to `Config` (e.g. `pub profiles:
ProfilesConfig`), do **all of the above for the section's fields, PLUS** these
four merge-wiring steps:

1. **Add the field to `Config`** and its `Default` impl.
2. **Add the field to `ConfigPartial`** as `Option<TheConfig>`.
3. **Add a merge arm to `Config::apply_partial`**:

   ```rust
   if let Some(profiles) = partial.profiles {
       self.profiles = profiles;
   }
   ```

   (Keybindings is the one exception — it merges its inner `HashMap`
   additively rather than replacing the whole section. Follow that pattern
   only for additive map-style sections.)

4. **Update the guard test** `every_config_section_survives_partial_merge`
   in `config.rs`: add a non-default mutation, an assertion, and the new
   field name to the trailing exhaustive `let Config { … } = loaded;`
   destructure.

## The guard test will catch you (use it)

`every_config_section_survives_partial_merge` is a deliberate tripwire with
two layers:

- **Compile-time:** its trailing `let Config { field: _, … } = loaded;` has
  **no `..` rest pattern**. Add a field to `Config` and this test stops
  compiling with `E0027: pattern does not mention field <name>` until you
  acknowledge it. That is the signal to do the four merge-wiring steps above.
- **Runtime:** it mutates one field per section, runs the real load path, and
  asserts each survived. A missing/broken `apply_partial` arm fails with
  `"<section> section dropped"`.

If you hit the `E0027` build error: **do not** just add `field: _` to silence
it. That defeats the guard. Add the `ConfigPartial` + `apply_partial` merge
wiring and a real assertion first, then the destructure entry.

## Serde gotchas

- `#[serde(skip_serializing_if = "Option::is_none")]` is fine — it omits only
  absent optionals. `#[serde(skip)]` is **not** fine for anything a user
  should be able to set; it drops the field from both load and save.
- `#[serde(rename_all = "kebab-case")]` / `"snake_case"` on an enum must match
  what you document in `config_example.toml`. Add a serialization test
  (`assert!(toml.contains("key = \"value\""))`) for any new enum.
- A new enum used as a config value needs `Serialize + Deserialize + Default`
  and a `#[serde(rename_all = …)]` consistent with the rest of the file.

## The Nix home-manager module mirror (`nix/home-manager-module.nix`)

Every config section must be settable through the home-manager module, or
managed installs cannot configure it. Mirroring a section there means three
edits in `nix/home-manager-module.nix`:

1. A `<section>Section` `let`-binding (a `lib.filterAttrs (_: v: v != null)`
   over the section's keys, so unset keys are omitted).
2. A `// lib.optionalAttrs (<section>Section != { }) { <section> = … }` arm in
   the `result` merge.
3. The `options.programs.freminal.settings.<section>` `mkOption`s — each
   `types.nullOr …` with `default = null` and a description naming the Rust
   default. Enums become `types.enum [ … ]` with the `#[serde(rename_all)]`
   string values.

Run `nixfmt --check`, `statix check`, and `deadnix --fail` on the module
afterward (see `nix-best-practices`). As of the `76.4c` audit + follow-up,
all sections through `notifications` are mirrored; keep it that way.

## When to stop and ask

- You want to skip the Nix module or Settings UI step "for now". Don't —
  surface the omission to the user explicitly and let them decide; a
  half-wired option is the exact bug this skill exists to prevent.
- The guard test's `E0027` is tempting you to add `field: _` without wiring
  the merge. Stop — wire `ConfigPartial` + `apply_partial` first.
- A new section needs non-replace merge semantics (like keybindings). Confirm
  the merge strategy with the user before copying the additive pattern.
