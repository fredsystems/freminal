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

Drives every OSC 99 (kitty desktop notification) code path: minimal/chunked
titles, title+body, update-by-id, base64 payloads, urgency, occasion,
sound, buttons, icons (by name, and by transmitted+cached data), app name,
auto-expiry, and the reverse-path reports (activation, close, `p=alive`
liveness poll, and the `p=?` capability handshake).

Run it **inside a freminal window** with `[notifications] enabled = true`
and `[notifications] osc_99 = true` in your config:

```sh
python3 test-scripts/osc99_notifications.py
```

The script presents a numbered menu (or `a` to run every step in order,
pausing between each). For each step it prints a description of what to
expect, the exact escape sequence being sent, and -- for the steps that
expect a reply from freminal (activation/close/alive/handshake) -- reads
back and prints the response bytes so you can diff them against
`Documents/KITTY_PROTOCOL_REFERENCE.md`.

Some steps ask you to unfocus or minimize the window, or click a
notification button, before the expected behavior appears.
