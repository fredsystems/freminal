#!/usr/bin/python3

"""Manual exerciser for OSC 99 (kitty desktop notifications).

Run this INSIDE a freminal window with `[notifications] enabled = true` and
`[notifications] osc_99 = true` in your config. This script is NOT a unit
test and is NOT wired into CI: it emits real OSC 99 escape sequences to your
terminal and, for the reverse-path steps, reads the response bytes freminal
writes back to the PTY (activation/close/alive/handshake reports). A human
has to watch the resulting desktop notifications (and, for a few steps,
unfocus or minimize the window, or click a notification button) to confirm
each code path against the spec.

Reference: `Documents/KITTY_PROTOCOL_REFERENCE.md`, "Desktop notifications
(OSC 99)" section -- envelope, metadata key table, payload types, report
formats, and the capability handshake are all documented there.

Usage:
    python3 test-scripts/osc99_notifications.py

The script presents a numbered menu. Pick a step (or `a` to run all steps in
order, pausing between each), watch/interact with the resulting desktop
notification, and compare what you see against the reference doc. Steps that
expect a reply from freminal (14-17) read stdin for a short window after
sending; if nothing arrives, the script prints a hint about what to check.

Response reading uses `termios`/`tty`/`select` and is POSIX-only. On
platforms without those modules (e.g. Windows) the send-only steps still
work; the reverse-path steps will note that response reading is unavailable
there.
"""

from __future__ import annotations

import base64
import sys
import time

try:
    import select
    import termios
    import tty

    _POSIX_TTY = True
except ImportError:  # pragma: no cover - Windows has no termios/tty
    _POSIX_TTY = False

ESC = "\x1b"
ST = "\x1b\\"  # String Terminator: ESC backslash

# A minimal valid 1x1 pixel PNG (black, no alpha), used for the icon-by-data
# test. Kept as a base64 constant so the script has zero binary payloads and
# no third-party PNG-writing dependency.
TINY_PNG_BASE64 = (
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk"
    "+A8AAQUBAScY42YAAAAASUVORK5CYII="
)


def b64(s: str) -> str:
    """Base64-encode a UTF-8 string, RFC 4648 standard alphabet."""
    return base64.b64encode(s.encode("utf-8")).decode("ascii")


def escaped(data: bytes) -> str:
    """Render bytes as a human-readable escaped string for display."""
    out = []
    for b in data:
        if b == 0x1B:
            out.append("\\x1b")
        elif 0x20 <= b < 0x7F:
            out.append(chr(b))
        else:
            out.append(f"\\x{b:02x}")
    return "".join(out)


def emit(metadata: str, payload: bytes = b"") -> None:
    """Build and write one OSC 99 escape sequence.

    `metadata` is the already-colon-joined `key=value` string (may be empty).
    `payload` is raw bytes (already base64-encoded by the caller if `e=1`
    was used in `metadata`).
    """
    sequence = f"{ESC}]99;{metadata};".encode("ascii") + payload + ST.encode("ascii")
    print(f"    sending: {escaped(sequence)}")
    sys.stdout.buffer.write(sequence)
    sys.stdout.buffer.flush()


def read_response(timeout: float = 3.0) -> bytes:
    """Read whatever bytes are available on stdin within `timeout` seconds.

    Puts the tty into raw mode so escape sequences freminal writes back
    (activation/close/alive/handshake reports) arrive unmangled. Always
    restores the original terminal mode. POSIX-only; returns b"" with a
    printed note on platforms without termios/tty/select.
    """
    if not _POSIX_TTY:
        print("    (response reading is POSIX-only; skipping on this platform)")
        return b""

    fd = sys.stdin.fileno()
    old_settings = termios.tcgetattr(fd)
    collected = bytearray()
    try:
        tty.setraw(fd)
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            ready, _, _ = select.select([fd], [], [], remaining)
            if not ready:
                break
            chunk = sys.stdin.buffer.read1(4096)  # type: ignore[attr-defined]
            if not chunk:
                break
            collected.extend(chunk)
            # Small grace period to catch the rest of a multi-byte sequence
            # that may arrive in a second read() burst.
            time.sleep(0.05)
    finally:
        termios.tcsetattr(fd, termios.TCSADRAIN, old_settings)
    return bytes(collected)


def print_response(data: bytes) -> None:
    if data:
        print(f"    received: {escaped(data)}")
    else:
        print(
            "    no response received (did you activate/dismiss the "
            "notification? is [notifications].enabled and osc_99 = true?)"
        )


def pause(prompt: str = "Press Enter to continue...") -> None:
    try:
        input(f"    {prompt}")
    except EOFError:
        pass


def step_header(number: int, title: str, description: str) -> None:
    print()
    print(f"=== Step {number}: {title} ===")
    print(f"    {description}")


# ---------------------------------------------------------------------------
# Individual test steps
# ---------------------------------------------------------------------------


def step_01_minimal_title() -> None:
    step_header(
        1,
        "Minimal title",
        "Empty metadata, plain-text title. Expect a notification titled "
        "'Hello world'.",
    )
    emit("", b"Hello world")


def step_02_title_and_body() -> None:
    step_header(
        2,
        "Title + body (separate chunks, same id)",
        "Title chunk (d=0) then body chunk (d=1) sharing i=n1. Expect one "
        "notification: title 'My Title', body 'My body text'.",
    )
    emit("i=n1:p=title:d=0", b"My Title")
    emit("i=n1:p=body:d=1", b"My body text")


def step_03_chunked_title() -> None:
    step_header(
        3,
        "Chunked title across multiple escapes",
        "Two title chunks sharing i=c1: d=0 holds display, d=1 finalizes "
        "and reassembles. Expect one notification titled "
        "'Part one part two'.",
    )
    emit("i=c1:p=title:d=0", b"Part one ")
    emit("i=c1:p=title:d=1", b"part two")


def step_04_update_by_id() -> None:
    step_header(
        4,
        "Update by id",
        "Send a finalized notification with i=u1, pause, then send another "
        "with the same i=u1. Expect the notification to update in place "
        "rather than create a second one.",
    )
    emit("i=u1", b"First version")
    pause("Look at the notification, then press Enter to send the update...")
    emit("i=u1", b"Second version")


def step_05_base64_payload() -> None:
    step_header(
        5,
        "base64 payload (e=1)",
        "Body sent as base64, including a non-ASCII character to prove "
        "UTF-8 round-trips. Expect body 'Base64 body \u2713'.",
    )
    body = "Base64 body \u2713"
    emit("i=b1:p=body:e=1", b64(body).encode("ascii"))


def step_06_urgency() -> None:
    step_header(
        6,
        "Urgency (u=0/1/2)",
        "Three notifications: low, normal, critical. Expect them to be "
        "visually distinguished by your notification daemon (if it "
        "supports urgency).",
    )
    emit("i=urg-low:u=0", b"Urgency: low (u=0)")
    emit("i=urg-normal:u=1", b"Urgency: normal (u=1)")
    emit("i=urg-critical:u=2", b"Urgency: critical (u=2)")


def step_07_occasion() -> None:
    step_header(
        7,
        "Occasion (o=unfocused / o=invisible / o=always)",
        "First: unfocus this window NOW, then press Enter -- expect the "
        "o=unfocused notification to appear because the source window "
        "lacks focus.",
    )
    pause("Unfocus the freminal window, then press Enter...")
    emit("i=occ-unfocused:o=unfocused", b"Occasion: unfocused")
    print(
        "    Next: minimize (or switch away from) this window, then press "
        "Enter -- expect the o=invisible notification to appear."
    )
    pause("Minimize/hide the freminal window, then press Enter...")
    emit("i=occ-invisible:o=invisible", b"Occasion: invisible")
    print("    Finally, o=always should appear regardless of focus state.")
    emit("i=occ-always:o=always", b"Occasion: always")


def step_08_sound() -> None:
    step_header(
        8,
        "Sound (s=)",
        "Sound is a hint the notification daemon may ignore entirely. "
        "First 'system' (platform default), then 'silent' (no sound).",
    )
    emit("i=snd-system:s=" + b64("system"), b"Sound: system")
    emit("i=snd-silent:s=" + b64("silent"), b"Sound: silent")


def step_09_buttons() -> None:
    step_header(
        9,
        "Buttons (p=buttons, a=report)",
        "A titled notification with three buttons: Yes / No / Maybe, "
        "separated by U+2028. Click a button and watch for the activation "
        "report (step 14 covers the report path in isolation, but a=report "
        "is set here too so you can see it live).",
    )
    emit("i=btn1:p=title:d=0", b"Pick one")
    labels = "Yes\u2028No\u2028Maybe"
    emit("i=btn1:p=buttons:a=report", labels.encode("utf-8"))
    print("    Click a button in the notification now if your daemon shows one.")
    data = read_response(timeout=8.0)
    print_response(data)


def step_10_icon_by_name() -> None:
    step_header(
        10,
        "Icon by name (n=)",
        "Two notifications, each referencing a standard icon name: "
        "'dialog-information' then 'error'.",
    )
    emit("i=icon-name1:n=" + b64("dialog-information"), b"Icon by name: info")
    emit("i=icon-name2:n=" + b64("error"), b"Icon by name: error")


def step_11_icon_by_data_and_cache() -> None:
    step_header(
        11,
        "Icon by data + g= cache",
        "A notification carrying a transmitted 1x1 PNG (p=icon, e=1, "
        "g=cachekey1), then a SECOND notification referencing g=cachekey1 "
        "alone to prove cache reuse (no icon bytes sent the second time).",
    )
    emit("i=icon-data1:p=title:d=0", b"Icon by data")
    emit(
        "i=icon-data1:p=icon:e=1:g=cachekey1",
        TINY_PNG_BASE64.encode("ascii"),
    )
    print("    Now sending a second notification reusing the cached icon...")
    emit("i=icon-data2:p=title:d=0", b"Icon from cache")
    emit("i=icon-data2:p=icon:g=cachekey1", b"")


def step_12_app_name() -> None:
    step_header(
        12,
        "App name (f=)",
        "Notification with an application name of 'My Test App'. Expect "
        "your notification daemon to display/group it under that name.",
    )
    emit("i=appname1:f=" + b64("My Test App"), b"App name test")


def step_13_auto_expiry() -> None:
    step_header(
        13,
        "Auto-expiry (w=)",
        "Three notifications: w=2000 (expires in ~2s), w=0 (never expires), "
        "w=-1 (OS default). Watch how long each stays visible.",
    )
    emit("i=exp-2000:w=2000", b"Expires in 2000ms")
    emit("i=exp-never:w=0", b"Never expires (w=0)")
    emit("i=exp-default:w=-1", b"OS default expiry (w=-1)")


def step_14_activation_report() -> None:
    step_header(
        14,
        "Activation report (a=report)",
        "Sends a notification with a=report:i=act1. Click on the "
        "notification body (whole-notification activation) and watch for "
        "freminal's report: an empty payload for whole-notification "
        "activation, or a 1-based button index if you click a button.",
    )
    emit("i=act1:a=report", b"Click me (whole-notification activation)")
    print("    Click the notification now...")
    data = read_response(timeout=8.0)
    print_response(data)


def step_15_close_report() -> None:
    step_header(
        15,
        "Close report (c=1)",
        "Sends a notification with c=1:i=cl1. Dismiss it (click the close "
        "button or let it expire) and watch for freminal's close report: "
        "'p=close' with an empty payload, or literal 'untracked' on "
        "platforms that cannot observe the close event (e.g. macOS).",
    )
    emit("i=cl1:c=1:w=5000", b"Dismiss me (close report)")
    print("    Dismiss the notification now...")
    data = read_response(timeout=10.0)
    print_response(data)


def step_16_alive_poll() -> None:
    step_header(
        16,
        "Alive poll (p=alive)",
        "Sends a liveness poll and reads freminal's reply: a comma-"
        "separated list of currently-live notification ids. Run this "
        "shortly after leaving a few notifications open (e.g. step 13's "
        "w=0 one) to see them listed.",
    )
    emit("i=alive1:p=alive", b"")
    data = read_response(timeout=5.0)
    print_response(data)


def step_17_capability_handshake() -> None:
    step_header(
        17,
        "Capability handshake (p=?)",
        "Sends a capability query and reads freminal's reply: colon-"
        "separated key=value capabilities. Diff this against the "
        "'Capability handshake' table in KITTY_PROTOCOL_REFERENCE.md.",
    )
    emit("i=q1:p=?", b"")
    data = read_response(timeout=5.0)
    print_response(data)


STEPS = [
    (1, "Minimal title", step_01_minimal_title),
    (2, "Title + body (separate chunks, same id)", step_02_title_and_body),
    (3, "Chunked title across multiple escapes", step_03_chunked_title),
    (4, "Update by id", step_04_update_by_id),
    (5, "base64 payload (e=1)", step_05_base64_payload),
    (6, "Urgency (u=0/1/2)", step_06_urgency),
    (7, "Occasion (o=unfocused/invisible/always)", step_07_occasion),
    (8, "Sound (s=)", step_08_sound),
    (9, "Buttons (p=buttons)", step_09_buttons),
    (10, "Icon by name (n=)", step_10_icon_by_name),
    (11, "Icon by data + g= cache", step_11_icon_by_data_and_cache),
    (12, "App name (f=)", step_12_app_name),
    (13, "Auto-expiry (w=)", step_13_auto_expiry),
    (14, "Activation report (a=report)", step_14_activation_report),
    (15, "Close report (c=1)", step_15_close_report),
    (16, "Alive poll (p=alive)", step_16_alive_poll),
    (17, "Capability handshake (p=?)", step_17_capability_handshake),
]


def print_menu() -> None:
    print()
    print("OSC 99 desktop notification exerciser")
    print("Run inside freminal with [notifications] enabled = true, osc_99 = true.")
    print()
    for number, title, _ in STEPS:
        print(f"  {number:>2}) {title}")
    print("   a) run all steps in order (pausing between each)")
    print("   q) quit")


def run_all() -> None:
    for number, title, fn in STEPS:
        print()
        print(f"--- Running step {number}: {title} ---")
        fn()
        if number != STEPS[-1][0]:
            pause("Press Enter for the next step...")


def main() -> None:
    if not _POSIX_TTY:
        print(
            "Note: termios/tty/select are unavailable on this platform "
            "(likely Windows). Send-only steps (1-13) still work; the "
            "reverse-path steps (14-17) will skip response reading."
        )

    while True:
        print_menu()
        try:
            choice = input("Select a step: ").strip().lower()
        except EOFError:
            break
        if choice in ("q", "quit", "exit"):
            break
        if choice == "a":
            run_all()
            continue
        matched = [fn for number, _, fn in STEPS if str(number) == choice]
        if not matched:
            print(f"Unrecognized choice: {choice!r}")
            continue
        matched[0]()


if __name__ == "__main__":
    main()
