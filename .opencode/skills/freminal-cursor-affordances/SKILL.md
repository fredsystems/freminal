---
name: freminal-cursor-affordances
description: Use ONLY when working in the freminal repository AND adding or debugging anything that changes the mouse cursor, or adding a new clickable/draggable element to the GUI — a chrome widget (button, menu entry, tab, checkbox, combo box, link), a hover affordance on the terminal surface (URL, command-block gutter, fold placeholder), or a drag handle (pane divider). Triggers on "the cursor doesn't change", "the hand only shows in one pane", "the cursor flickers while dragging", "the resize arrow region is smaller than the grab region", "hovering my new button shows a plain arrow", or any use of set_cursor_icon / output.cursor_icon / on_hover_cursor. Codifies the single-writer rule for output.cursor_icon and the per-widget-type affordance rules.
---

# Freminal: `output.cursor_icon` has exactly one writer per frame

`egui::PlatformOutput::cursor_icon` is **one field for the whole window**, and
egui resets it to `Default` at the start of every frame. `Context::set_cursor_icon`
and `output_mut(|o| o.cursor_icon = ..)` are both plain unconditional writes with
**no priority mechanism whatsoever**:

```rust
pub fn set_cursor_icon(&self, cursor_icon: CursorIcon) {
    self.output_mut(|o| o.cursor_icon = cursor_icon);   // that is the entire body
}
```

So when two pieces of code both want a say, **whichever runs later in the frame
silently wins**. Nothing warns you. The losing call looks completely correct at
its own call site.

This has produced four separate bugs in this repo (issues #462, #493). Every one
of them was "I wrote the correct cursor and nothing happened".

## The rule

> Compute what the cursor should be, resolve the contenders with an explicit
> precedence, and write it **once**. Never write `cursor_icon` speculatively and
> hope you run last.

---

## Part 1 — chrome widgets (menus, buttons, tabs, modals)

### Buttons are handled globally — do nothing

`freminal/src/gui/chrome_style.rs` sets:

```rust
visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);
```

egui consults this from `Button`'s own paint path, so **anything built on
`egui::Button` gets the pointing hand for free** — `ui.button`,
`ui.small_button`, `ui.add(Button::new(..))`, `ui.add_enabled(.., Button::new(..))`,
menu entries, and the `ComboBox` header.

Do **not** add `.clickable()` to those. It is redundant.

### Everything else must opt in explicitly

egui only honours `interact_cursor` from `Button`. These widgets are **not**
covered and each needs `.clickable()`:

| Widget | Covered by `interact_cursor`? |
| ------------------------- | ----- |
| `ui.button` / `small_button` / `Button::new` | yes — do nothing |
| `ui.menu_button`, `ComboBox` header | yes — do nothing |
| `ui.selectable_value` | **no** — add `.clickable()` |
| `ui.selectable_label` | **no** — add `.clickable()` |
| `ui.checkbox` | **no** — add `.clickable()` |
| `ui.radio` / `radio_value` | **no** — add `.clickable()` |
| `ui.toggle_value` | **no** — add `.clickable()` |
| `ui.hyperlink` / `hyperlink_to` / `link` | **no** — add `.clickable()` |
| anything hand-rolled via `ui.interact(..)` | **no** — add `.clickable()` |

Use the vocabulary in `freminal/src/gui/hover_cursor.rs`:

```rust
use super::hover_cursor::HoverAffordance;

ui.checkbox(&mut cfg.ligatures, "Enable Ligatures").clickable();
ui.selectable_value(&mut cfg.mode, ThemeMode::Dark, "Dark").clickable();
ui.add_enabled(can_save, save_widget).clickable_when(can_save);
```

Three methods, naming intent rather than appearance:

- `.clickable()` — pointing hand. Anything that performs an action on click.
- `.disabled_affordance()` — `NotAllowed`. Present but not actionable; a hand
  on something inert is a lie.
- `.clickable_when(enabled)` — picks between the two. Pairs with `add_enabled`.

### What must NOT get a hand

- **Text fields.** egui already gives `TextEdit` an I-beam, which correctly
  says "type here", not "click here". Leave it.
- **Drag handles.** They get a directional resize cursor so the drag axis is
  visible. See Part 3.
- **Plain labels.** Obviously.

---

## Part 2 — the terminal surface

Terminal panes do **not** use `HoverAffordance`. They have their own resolution
in `freminal/src/gui/terminal/widget.rs`, because they must additionally
arbitrate against the application's OSC 22 pointer shape.

```rust
let pointer_hover = PointerHover {
    command_block_gutter: gutter_hovered,
    fold_placeholder: placeholder_hovered,
    url: cache.cached_hovered_url.is_some(),
};
```

`PointerHover::resolve()` reduces those to a single `PointerTarget`, ordered by
descending precedence: **gutter → fold placeholder → URL → terminal content**.
Only `TerminalContent` defers to OSC 22. `cursor_icon_for` maps the winner to
an icon, and it is written exactly once.

**Adding a new hoverable thing on the terminal surface?** Add a field to
`PointerHover`, a variant to `PointerTarget` at the right precedence position,
and an arm to `cursor_icon_for`. Do not add another `set_cursor_icon` call.

### Only the pane under the pointer may write

Every pane runs this code every frame. An unconditional write means the **last
pane rendered decides the cursor for the entire window** — which presents as
"the hand only works in the bottom pane of a split" and, worse, stamps over the
cursors egui set for its own chrome.

```rust
if ui.rect_contains_pointer(pane_rect) && split_border_hover == SplitBorderHover::Clear {
    ui.ctx().output_mut(|o| o.cursor_icon = resolved_icon);
}
```

`rect_contains_pointer` respects layer and clip rect, so a modal drawn above the
pane keeps its own cursor. It does **not** account for same-layer overlaps —
hence the explicit `SplitBorderHover` term, see Part 3.

---

## Part 3 — drag handles (pane dividers)

Dividers are the awkward case and account for two of the four bugs.

**Their hit sensor is intentionally wider than the thing it moves** (3px either
side of a 1px line), so the pointer sits *geometrically inside an adjacent pane*
while *logically over chrome*. Sensors are not a separate egui layer, so
`rect_contains_pointer` cannot see them. They must be passed to the pane
explicitly as `SplitBorderHover` so it abstains.

Two invariants, both learned the hard way:

1. **Drive the abstain-gate from the same condition that writes the cursor.**
   The sensor rects are built from `borders`, computed at the top of the frame
   and therefore *one frame behind* the divider that `resize_split` is actively
   moving. During a drag the pointer routinely runs ahead of the stale rect. A
   gate that independently hit-tests the published rects will disagree with the
   write exactly when it matters, and the cursor flickers. Set a flag where the
   cursor is applied (`response.hovered() || response.dragged()`), and gate on
   that flag — plus `border_drag_active`, so an in-flight drag keeps the cursor
   even on a frame where the rect has fallen behind entirely.

2. **Request a repaint when the drag ends.** `drag_stopped` applies the final
   resize, but this frame's rects predate it and nothing else schedules another
   frame — so the cursor drops to the default arrow with the pointer still over
   the divider. Call `ctx.request_repaint()` on release.

The drag cursor stays the **directional resize arrow** for the whole gesture,
rather than switching to `Grabbing`. That is a deliberate product decision: it
matches GNOME/KDE/VS Code/browser splitters and keeps the drag axis visible.

---

## How to verify

There is no automated test for the applied cursor — it needs a live egui
context and a real pointer. The *decision* logic is unit-tested
(`pointer_target_tests`, `input_suppressors_tests` in `widget.rs`); extend those
when you change precedence. The rest is manual, and these are the checks that
actually catch the bugs above:

1. Hover the new element — cursor changes.
2. **Do it in a split, in every pane**, not just one. Single-pane and
   bottom-pane both mask the multi-writer bug.
3. Move from the element onto plain terminal text — the cursor reverts. A
   *stuck* cursor is as much a bug as an absent one.
4. For a drag handle: the cursor must change at the same moment the handle
   becomes grabbable (regions match), hold steady for the whole drag (no
   flicker), and persist after release.
5. With an app that sets OSC 22 running in one pane, confirm each pane keeps its
   own shape.

## When to stop and ask

- You want a *new* precedence order on the terminal surface (e.g. URL should
  outrank the gutter). That is a product decision, not an implementation
  detail — confirm it.
- You believe a widget needs a cursor other than hand / not-allowed / I-beam /
  resize. Surface it rather than inventing a fourth vocabulary word.
- You are tempted to write `cursor_icon` from a new location. Almost certainly
  the answer is a new contender in an existing resolution, not a new writer.
