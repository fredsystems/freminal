---
name: freminal-toast-options
description: Use ONLY when working in the freminal repository AND adding, changing, or reasoning about a notification/toast source and how it is routed — a new freminal-derived toast (clipboard, layout, recording, paste-blocked, config-reload, and future UI-feedback toasts), a new OSC-sourced notification (OSC 9/777/99/133D), or a new toast placement. Codifies the two-tier routing model: the restricted Toast/Disabled `FreminalToastRouting` for freminal's own UI toasts (no system leg, no master-switch gate) versus the full `NotificationRouting` (Toast/System/Both/SystemWhenUnfocused/Disabled) for terminal-application-driven notifications, plus the `FreminalToastCategory` → `routing_<cat>` → `route_freminal_toast` → `push_positioned(ToastPlacement)` consumer chain and where the resize overlay deliberately sits outside it.
---

# Freminal: toast + notification routing options

Freminal has **two distinct notification models**, and choosing the
wrong one is the most common mistake when adding a new notification
source. This skill names the two models, the decision rule between
them, and the exact wiring each requires. It pairs with
`freminal-config-options` (the generic config-field checklist) and
`freminal-modal-input-suppression` (toasts are not typing modals).

All file references are to the freminal repo. The routing enums live
in `freminal-common/src/config.rs`; the router lives in
`freminal/src/gui/notifications.rs`; placement lives in
`freminal/src/gui/toast.rs`.

## The two models

| Aspect        | Terminal-app notifications                   | Freminal-derived toasts                    |
| ------------- | -------------------------------------------- | ------------------------------------------ |
| Source        | OSC 9 / 777 / 99 / 133D, command-finished    | freminal's own UI events                   |
| Routing enum  | `NotificationRouting` (5 variants)           | `FreminalToastRouting` (2 variants)        |
| System leg    | Yes — can raise a desktop notification        | No — toast or nothing, ever                |
| Master switch | Gated by `notifications.enabled`             | Bypasses `enabled` entirely                |
| Focus-aware   | Yes (`wants_toast(focused)`)                 | No (`wants_toast()`, no argument)          |
| Router entry  | `NotificationRouter::route` / `route_osc99`  | `NotificationRouter::route_freminal_toast` |
| Default anchor| `ToastPlacement::TOP_RIGHT`                  | window- or pane-centered                   |

### Model 1 — terminal-application notifications (full options)

These come from the program running in the terminal (an OSC escape
sequence) or from a freminal-observed command completion. The user
gets the full `NotificationRouting` choice:

```rust
pub enum NotificationRouting {
    Toast,               // in-app toast only
    System,              // desktop notification only
    Both,                // toast + desktop notification
    SystemWhenUnfocused, // desktop when unfocused, toast when focused
    Disabled,            // suppressed entirely
}
```

`wants_toast(focused)` and `wants_system(focused)` are both
focus-aware. `route` / `route_osc99` gate on `config.enabled` first,
then branch on both legs.

### Model 2 — freminal-derived toasts (Toast / Disabled only)

These are freminal's **own** UI feedback (you copied text, a layout
saved, recording started). They never leave the window as a desktop
notification, so the only sensible choices are "show a toast" or
"don't":

```rust
pub enum FreminalToastRouting {
    Toast,    // in-app toast (default)
    Disabled, // suppressed
}
```

`wants_toast()` takes no `focused` argument. `route_freminal_toast`
does **not** check `config.enabled` and has **no** `wants_system` /
`show_system` leg — these toasts are governed solely by their own
per-category `routing_<category>` field.

## The decision rule

Ask: **does the program running in the terminal cause this, or does
freminal's own UI cause it?**

- Program-caused (an escape sequence, a finished command) → Model 1,
  `NotificationRouting`, wired into `route` / `route_osc99`.
- Freminal-UI-caused (a keypress, a menu action, a config reload) →
  Model 2, `FreminalToastRouting`, a new `FreminalToastCategory`
  variant, wired through `route_freminal_toast`.

If you are tempted to give a freminal-derived toast a `System` option,
stop: the maintainer's explicit design (issue #433) is that
freminal's own toasts are toast-or-nothing. Adding a system leg to
them is a design change, not an implementation detail — surface it.

## The freminal-derived consumer chain (Model 2)

Every freminal-derived toast flows through the same four stages. When
adding a new one, wire all four:

1. **Category** — add a variant to `FreminalToastCategory`
   (`config.rs`). The six today are `ClipboardCopy`, `ClipboardRemote`,
   `Layout`, `Recording`, `PasteBlocked`, `ConfigReload`.

2. **Config field** — add a `routing_<cat>: FreminalToastRouting`
   field to `NotificationsConfig`, and add its arm to
   `NotificationsConfig::routing_for_category` (the `match category`
   dispatch). This is a config option: the full
   `freminal-config-options` checklist applies (Default,
   `config_example.toml`, Nix module `mkOption` + known-keys list,
   Settings UI row, round-trip test). Because `NotificationsConfig` is
   merged as a whole section in `apply_partial`, adding a field inside
   it needs no `apply_partial` change — but everything else in the
   checklist still applies.

3. **Router** — the push site calls
   `NotificationRouter::route_freminal_toast(category, kind, title,
   detail, placement, config, toasts)` (usually via the
   `FreminalGui::route_freminal_toast` `&self` wrapper in `mod.rs`,
   which handles the `RefCell` borrow). The router checks
   `config.routing_for_category(category).wants_toast()` and, if true,
   calls `toasts.push_positioned(kind, title, detail, placement)`.

4. **Placement** — pick a `ToastPlacement` (see below).

## Placement

`ToastPlacement { position: ToastPosition, origin: Option<(WindowId,
PaneId)> }`. Three consts / constructors:

- `ToastPlacement::TOP_RIGHT` — stacked top-right. Used by the
  severity pushers (`ToastStack::error` / `info`) and every Model 1
  (OSC / error / info) toast.
- `ToastPlacement::WINDOW_CENTERED` — centered in the window. The
  default for freminal-derived toasts not tied to one pane (recording,
  layout, config reload, remote clipboard, paste-blocked).
- `ToastPlacement::pane_centered(window_id, pane_id)` — centered in
  the originating pane. Used only where the event belongs to a
  specific pane (today: "Copied to clipboard"). Falls back to
  window-centered when that pane/window is not the one being rendered.

Rule of thumb: a pane-specific event (something you did *in* a pane)
is `pane_centered`; a window/app-level event is `WINDOW_CENTERED`;
program-driven notifications are `TOP_RIGHT`.

There is no `ToastStack::warning` helper — it was deleted in slice 2
as dead code. Push a warning-severity toast via
`route_freminal_toast(..., ToastKind::Warning, ...)` /
`push_positioned(ToastKind::Warning, ...)`, not a bespoke wrapper.
Do not reintroduce a dead severity wrapper; if a new `ToastPosition`
or placement is genuinely needed it must have a real caller and a
test (no `#[allow(dead_code)]`).

## The resize overlay is NOT a toast

The window-resize "cols × rows" readout looks like a toast but is
deliberately **not** one. It is a passive, hand-painted egui HUD
(`ResizeOverlayState` in `window.rs`, drawn inline in `app_impl.rs`),
gated directly by `config.notifications.show_resize_overlay` — it
never touches `ToastStack`, `route_freminal_toast`, or
`push_positioned`. It has its own linger/fade timing.

Why it matters: a whole-window size readout that stacked as a toast
would fight the real toasts for the corner and re-trigger on every
resize tick. Keeping it a separate centered overlay was the
issue #433 design. The **only** place the two concepts intersect is
frame damage: both the resize overlay and any live toast set
`toast_active` (in `app_impl.rs`) so their self-animating region
forces a full-frame present. If you add another self-animating
passive overlay, wire it into that same `toast_active` decision — but
do not route it as a toast.

## Common mistakes this skill prevents

- Giving a freminal-derived toast a `System`/`Both` routing option
  (wrong model — surface it as a design change).
- Gating a freminal-derived toast on `notifications.enabled` (they
  bypass it by design).
- Adding a `routing_<cat>` field but forgetting the
  `routing_for_category` arm (compiles, but the category silently
  can't be configured).
- Calling `wants_toast(focused)` on a `FreminalToastRouting` (it takes
  no argument) or `wants_toast()` on a `NotificationRouting` (it
  requires `focused`).
- Routing the resize overlay (or a future passive HUD) as a toast.
- Reintroducing a `ToastStack::warning`-style dead wrapper.

## When to stop and ask

- A new notification source doesn't cleanly fit either model (e.g. a
  freminal-UI event that genuinely should also raise a desktop
  notification). The two-model split is a maintainer design decision —
  surface the ambiguity rather than bolting a system leg onto Model 2.
- A new toast needs a placement none of the three consts cover. A new
  `ToastPosition` variant is a layout change with its own tests and
  grouping/occlusion implications (see the coincident-group handling
  in `toast.rs`); confirm the need first.
