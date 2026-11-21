---
name: freminal-reply-path
description: Use ONLY when working in the freminal repository AND adding or changing anything that writes bytes back to the application running in the terminal -- a reply to a query (DA, DSR, CPR, DECRQM/DECRPM, DECRQSS, XTGETTCAP, XTVERSION, CSI t reports, OSC 4/10-19/22/52/99 queries, kitty graphics replies, CSI ? u), a GUI-originated report (window state, title, clipboard, OSC 99 activation/close/alive), or a generated key, mouse, focus or bracketed-paste sequence. Codifies handler framing (S8C1T-aware, never tmux-wrapped), the typed GuiReply path for GUI-side replies, PTY-thread ordering before a following DA1, the bounded reply queue, and the Introducer helper for generated CSI/SS3.
---

# Freminal: the reply path

Every byte freminal sends to the child that is not the user's own typing is a reply or a
generated sequence. Task 130 gave them one path; Tasks 147 and 148 made it 8-bit-aware and
bounded. Breaking any rule below either corrupts the application's input or lets output
amplify into unbounded memory.

## Rules

- **Replies are framed by the handler.** Build the body only and send it with
  `write_csi_response`, `write_osc_response`, `write_dcs_response` or `write_apc_response`
  (`terminal_handler/pty_writer.rs`). They choose 7-bit or 8-bit (S8C1T) introducers and the
  terminator. Never hand-format `\x1b[`, `\x1b]` or `\x1bP` in a reply. `write_to_pty` is
  reserved for the VT52 `ESC / Z` reply and ENQ.
- **Replies are never tmux-wrapped.** tmux reads the outer terminal's replies as key input and
  never unwraps `DCS tmux;`. A reply to a query that arrived through tmux passthrough is sent
  plain, like kitty, WezTerm and Ghostty do.
- **Answer on the PTY thread, in stream order.** A capability or state query is answered while
  processing its own output, so the reply precedes the reply to a later query in the same read
  (applications detect support by sending the query followed by DA1). Do not defer a reply to
  the GUI if the PTY thread can know the answer; host facts it cannot know reach it through
  `HostCapabilities`.
- **GUI-originated replies are typed.** The GUI never formats bytes. It sends
  `InputEvent::Reply(GuiReply::...)` (`freminal-terminal-emulator/src/io/gui_reply.rs`) and the
  handler serialises it (`write_gui_reply`), framing and sanitising it. Text that came from the
  application (titles, selection names, ids) is stripped of control bytes before it is echoed
  (the title-report injection class, CVE-2003-0063). Long-lived GUI holders of a reply handle
  keep a `Weak`, so they cannot keep a closed pane's shell alive.
- **The reply queue is bounded.** Replies travel on their own bounded channel
  (`REPLY_QUEUE_CAPACITY` messages, `MAX_QUEUED_REPLY_BYTES` bytes); a full queue drops and
  counts the reply. Keystrokes travel on the unbounded input channel and are written first. GUI
  report requests are capped per pane (`MAX_OUTSTANDING_REPORTS`). Do not route a new reply
  around these bounds, and do not put a control message (a resize) on the reply queue.
- **Generated sequences honour S8C1T.** Key, focus, mouse and bracketed-paste encoders emit
  their introducer through `Introducer { Csi, Ss3 }` (`freminal-terminal-emulator/src/input.rs`)
  at the construction site. Never rewrite already-encoded bytes (Alt+`[` legitimately sends
  `ESC [`). VT52 mode and Alt-prefixed text are unaffected.
- **Not recorded to FREC.** Replies are terminal output, not user input.

## Tests a reply change needs

- The exact bytes in 7-bit and in 8-bit mode.
- Ordering against a following DA1 in the same read, through `handle_incoming_data`.
- For application-supplied text: a value containing ESC, BEL, C1 (U+0080-U+009F) and DEL
  produces a reply containing none of them.
- For tmux passthrough: the reply is unwrapped.

## When to stop and ask

- A reply's content depends on GUI state the PTY thread cannot see and that `HostCapabilities`
  does not model.
- The change would need the GUI to block on a reply, or a lock between threads
  (`freminal-architecture`).
