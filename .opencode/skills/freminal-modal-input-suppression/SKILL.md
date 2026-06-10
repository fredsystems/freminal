---
name: freminal-modal-input-suppression
description: Use ONLY when working in the freminal repository AND adding, editing, or debugging a GUI modal, dialog, overlay, popup, or any in-window widget that contains a focusable input (a TextEdit, search box, name field, filter box, etc.). Triggers on "my dialog doesn't accept typing", "focus bounces back to the terminal", "the modal renders but I can't type in it", "Escape/Enter does nothing in the dialog", or creating a new egui::Window/Area on the terminal surface. Codifies the ui_overlay_open registration + lock_focus(true) + per-frame request_focus pattern that every working modal uses, and that every new modal forgets.
---

# Freminal: a new modal must register with `ui_overlay_open` or it can't be typed in

This is the single most-repeated GUI bug in freminal. A new modal/overlay
renders fine, but **keystrokes go to the terminal (the PTY) instead of the
modal's text field** — focus appears to "bounce back" to the terminal. It has
been (re)fixed for the settings modal, the buffer-search overlay, the
command-history palette, the tab-rename editor, the save-layout prompt, and the
paste-guard confirm dialog (Task 77).

The cause is always the same: the terminal widget aggressively grabs egui
keyboard focus and forwards every key event to the PTY. Unless the new modal
**opts the active pane out of that behaviour**, the modal's `TextEdit` never
keeps focus.

## The two-part fix (do both, every time)

### Part 1 — register the modal in `ui_overlay_open`

`FreminalGui::update()` computes a single boolean,
`ui_overlay_open`, in `freminal/src/gui/app_impl.rs` (search for
`let ui_overlay_open =`). It currently OR-s together every overlay's
"is open" predicate:

```rust
let ui_overlay_open = any_menu_open
    || self.pending_save_layout.is_some()
    || self.about_window_open
    || self.welcome.is_open()
    || win.renaming_tab.is_some()
    || win.paste_dialog.is_open();   // each modal adds its own line
```

**Add your modal's open-predicate to this expression.** That is the whole
gate. `ui_overlay_open` is passed into every pane's
`terminal_widget.show(...)`, where it becomes `suppress_input` (in
`freminal/src/gui/terminal/widget.rs`, search `let suppress_input =`). When
`suppress_input` is true the widget:

1. **skips the focus-lock grab** — the block that calls
   `response.request_focus()` + `set_focus_lock_filter(...)` on the
   `terminal_focus` id is gated on `!suppress_input`. Skipping it releases the
   terminal's hold on Tab / arrows / Escape so egui can route them to your
   modal.
2. **bypasses `write_input_to_terminal` entirely** — the big
   `if suppress_input || ... { /* reset, send nothing */ } else { ui.input(... write_input_to_terminal ...) }`
   block means the PTY receives no key or mouse events that frame.

There is a deliberate **one-frame trailing suppression**
(`overlay_was_open_last_frame`) so the click that dismisses a modal does not
leak to the terminal as a pointer event. You do not need to touch it; just
make sure your open-predicate flips to `false` the frame the modal closes.

A few overlays gate at the pane level instead of through `ui_overlay_open`
(buffer search and command history are checked directly as
`view_state.search_state.is_open` / `view_state.command_history.is_open` inside
`widget.rs` because their state lives on `ViewState`, not `PerWindowState`).
Either location works; the rule is that **some predicate the widget can see
must be true while your modal is open**. For a modal whose state lives on
`PerWindowState` or `FreminalGui`, `ui_overlay_open` is the right place.

### Part 2 — make the modal's `TextEdit` hold focus

Registering with `ui_overlay_open` stops the terminal from stealing focus, but
your field still has to _take and keep_ it. Requesting focus once on open is
**not enough** — egui can hand focus back. Mirror the search / command-history
pattern exactly:

```rust
let response = ui.add(
    egui::TextEdit::multiline(&mut state.edit_buffer)   // or ::singleline
        .font(egui::TextStyle::Monospace)
        .desired_width(f32::INFINITY)
        // Hold focus so Tab / clicks inside the dialog don't bounce focus away.
        .lock_focus(true),
);
// Pull focus EVERY frame it lacks it — not just on the first frame.
if !response.has_focus() {
    response.request_focus();
}
```

Reference implementations to copy from:

- `freminal/src/gui/search.rs` — `show_search_bar` (`.lock_focus(true)` +
  per-frame `request_focus`).
- `freminal/src/gui/command_history.rs` — `show_command_history_palette` (same
  pattern, plus consuming Up/Down/Enter/Escape via `ui.input` so they never
  reach the PTY).

Do **not** keep a `just_opened` / `just_entered_edit` bool to request focus
"only once" — that is the anti-pattern that causes the bounce. The per-frame
`if !has_focus { request_focus() }` is the correct idiom and makes such a flag
dead state (clippy will flag it).

## Keyboard shortcuts inside the modal

Read Escape / Enter / Ctrl+Enter via `ctx.input(|i| i.key_pressed(...))` (or
`ui.input`). Because the terminal's focus-lock is released (Part 1) and the
PTY is not being fed (Part 1), those keys are free for the modal. Resolve
button clicks before keyboard shortcuts in the same frame so a click wins if
both happen.

## How to verify

1. Build and run. Open the modal.
2. Type into its field — characters must appear in the field, **not** in the
   terminal behind it.
3. Press Escape — the modal cancels; the terminal does not receive an Escape.
4. With a text field focused, press Tab — focus stays in the field
   (that is what `lock_focus(true)` buys you).

There is no automated test for the full focus path (it needs a live egui
context). The boolean state machine for the one-frame trailing suppression is
unit-tested in `widget.rs` (`mod overlay_suppress_input_tests`,
`suppress_input_state_machine`); extend or mirror that if you change the
suppression logic itself, but the per-modal registration is verified manually.

## Why this keeps happening (and the smell to watch for)

Nothing in the type system forces a new modal to register with
`ui_overlay_open`. A modal added without it compiles, renders, and looks
finished — the bug only shows when you try to type. **Whenever you add an
`egui::Window` / `egui::Area` / popup on the terminal surface that contains a
focusable widget, treat "register in `ui_overlay_open`" and "lock_focus +
per-frame request_focus" as part of the definition of done**, the same way a
new config option is not done until it is wired through `ConfigPartial` (see
`freminal-config-options`).

## When to stop and ask

- Your modal's open-state lives somewhere the terminal widget cannot see
  (neither `PerWindowState`, `FreminalGui`, nor `ViewState`). Surface this —
  the gate needs a predicate the widget can read.
- You believe the modal genuinely should let some keys fall through to the
  terminal (a rare, deliberate design). Confirm with the user; the default is
  full suppression.
