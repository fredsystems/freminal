# test-scripts

Manual exercisers for protocol code paths that are impractical to fully
verify with `cargo test` alone -- things that need a human to watch a real
desktop notification, unfocus a window, or click a button. These scripts are
**not** wired into CI and are **not** a substitute for the automated
`cargo test` suite; they complement it by giving a human a repeatable,
documented way to walk every code path of a spec and compare freminal's
behavior against the reference.

Nothing here is Rust or gets built by `cargo`/`xtask`. Each script is a
standalone, dependency-free Python 3 file.

## osc99_notifications.py

Walks through every OSC 99 (kitty desktop notification) code path as
implemented by freminal after Task 138: minimal, chunked, id-less and base64
payloads, the 1024-character cut, update in place, `p=close`, activation and
close reports, buttons, the `focus` action, urgency, sounds, the eight
standard icon names, a generated 64x64 PNG with `g=` cache reuse and
icon-only priming, app name and category, expiry, occasions (window unfocused
and background tab), `p=alive`, the `p=?` capability handshake (compared
against the reference replies), `osc99_filters`, and controls answered
mid-transfer.

Run it **inside a freminal window** with `[notifications] enabled = true`
and `[notifications] osc_99 = true` in your config:

```sh
python3 test-scripts/osc99_notifications.py
```

The script presents a numbered menu (`a` runs every step, pausing between
them; `q` quits). Each step prints what to expect, the exact escape sequence
it sends, and -- for steps that expect a reply from freminal -- reads the
reply with the tty in no-echo mode and prints it decoded, one per line.
Replies that arrive late (for example a click on an older notification) are
shown as "late reply:" instead of corrupting the menu input. Steps that need
the window unfocused or in a background tab use a visible countdown instead of
asking for a keypress.

All "Expect:" lines assume the default `routing_osc99 = "both"` on
Linux/BSD (toast plus desktop notification); the script's docstring lists how
other routings and macOS/Windows differ.

`--dry-run` prints every sequence of every step without touching the tty and
checks each one is well-formed (metadata grammar, chunk limits, base64, the
chunked icon reassembling to a valid PNG):

```sh
python3 test-scripts/osc99_notifications.py --dry-run
```

Reply reading is POSIX-only (`termios`); elsewhere the script still sends and
says that reading is skipped.

## kitty_graphics.py

Drives the full kitty graphics protocol spec set (Task 100): transmit /
put / delete / query, animation (frame transfer, run/stop/loop, compose),
image-number (`I=`) references, unicode placeholders, relative placements
(including the `ENOPARENT` / `ECYCLE` / `ETOODEEP` error paths), `o=z` zlib
compression, `t=s` shared memory, source-rect crop, and a light storage-
quota exercise.

Run it **inside a freminal window**:

```sh
python3 test-scripts/kitty_graphics.py
```

The script presents a numbered menu (or `a` to run every step in order,
pausing between each). For each step it prints a description of what to
expect on screen, the exact wire bytes being sent (with the base64 payload
included), and -- for steps that expect a reply from freminal -- reads
back and prints the response bytes so you can diff them against
`Documents/KITTY_PROTOCOL_REFERENCE.md`, "Graphics protocol completion".

Most steps are self-contained and only require watching the terminal for
an image to appear, move, animate, or vanish; a few (the relative
placement error cases, the animation run/stop pair) ask you to pause and
observe before continuing.
