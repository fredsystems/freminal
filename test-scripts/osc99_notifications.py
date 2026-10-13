#!/usr/bin/env python3

"""Manual exerciser for OSC 99 (kitty desktop notifications).

Run this INSIDE a freminal window with `[notifications] enabled = true` and
`[notifications] osc_99 = true` in your config. This script is NOT a unit
test and is NOT wired into CI: it emits real OSC 99 escape sequences to your
terminal and, for the reverse-path steps, reads the reports freminal writes
back to the PTY (activation / close / alive / capability replies). A human
has to watch the resulting toasts and desktop notifications (and, for a few
steps, switch window/tab or click a notification) to confirm each code path.

Reference: `Documents/KITTY_PROTOCOL_REFERENCE.md`, "Desktop notifications
(OSC 99)" -- envelope, key table, report formats, the `p=?` handshake and
freminal's documented differences from kitty are all there. Config keys are
documented under `[notifications]` in `config_example.toml`.

ROUTING ASSUMED BY EVERY "Expect:" LINE
    The expectations assume the default `routing_osc99 = "both"` on
    Linux/BSD: each notification is shown as an in-app TOAST and as a desktop
    notification through the freedesktop daemon (D-Bus). Other setups differ:

    toast                  toast only: no desktop notification, no sounds,
                           a transmitted icon image is never shown (standard
                           icon names show as glyphs).
    system                 desktop notification only: no toast. Activation,
                           buttons and focus need the daemon's `actions`
                           capability; named sounds need its `sound` one.
    system_when_unfocused  the toast while freminal's window is focused, the
                           desktop notification while it is not.
    disabled / osc_99 off  freminal does not speak OSC 99 at all: nothing is
                           shown and `p=?` goes unanswered.
    macOS / Windows        the system leg is UNTRACKED: no activation or close
                           reports from it, no update in place, no buttons, no
                           programmatic close, no expiry enforced by freminal.
                           A `c=1` request is answered immediately with
                           `p=close;untracked` when the notification is shown.
                           (With `both`, the toast leg still tracks.)

Usage:
    python3 test-scripts/osc99_notifications.py            # interactive menu
    python3 test-scripts/osc99_notifications.py --dry-run  # print and check
                                                           # every sequence,
                                                           # touch no tty

The menu lists numbered steps (pick one, `a` runs them all with pauses, `q`
quits). Steps that expect a reply from freminal put the tty into no-echo mode
BEFORE emitting, read the replies, print each decoded on its own line and
restore the tty. Replies that arrive late (for example when you click a
notification after a step finished) are drained before each prompt and shown
as "late reply:", so they never end up echoed into the menu input.

Reply reading uses `termios`/`select` and is POSIX-only. On platforms without
them (e.g. Windows) the send-only steps still work; reading is skipped with a
note.
"""

from __future__ import annotations

import argparse
import base64
import colorsys
import contextlib
import inspect
import os
import re
import struct
import sys
import time
import zlib
from collections.abc import Callable, Iterable, Iterator
from dataclasses import dataclass, field

try:
    import select
    import termios

    _POSIX_TTY = True
except ImportError:  # pragma: no cover - Windows has no termios
    _POSIX_TTY = False

ESC = b"\x1b"
ST = b"\x1b\\"  # String Terminator: ESC backslash
U2028 = "\u2028"  # button label separator

# Per-chunk payload limits from the spec.
MAX_PLAIN_CHUNK = 2048  # bytes before encoding
MAX_B64_CHUNK = 4096  # bytes after encoding


@dataclass
class Options:
    """Process-wide run mode."""

    dry_run: bool = False
    sent: list[bytes] = field(default_factory=list)


OPTS = Options()


# ---------------------------------------------------------------------------
# Encoding helpers
# ---------------------------------------------------------------------------


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


def osc(metadata: str, payload: bytes | str = b"") -> bytes:
    """Build one OSC 99 sequence: `ESC ] 99 ; <metadata> ; <payload> ESC \\`.

    `metadata` is the already colon-joined `key=value` string (may be empty;
    both semicolons are always present). `payload` is raw bytes or a str that
    is UTF-8 encoded; when `e=1` is in `metadata` the caller passes base64.
    """
    if isinstance(payload, str):
        payload = payload.encode("utf-8")
    return b"\x1b]99;" + metadata.encode("ascii") + b";" + payload + ST


def b64_chunks(data: bytes, size: int = MAX_B64_CHUNK) -> list[bytes]:
    """Base64-encode `data` and split the text into <= `size` byte chunks.

    `size` is a multiple of 4, so every chunk is independently decodable and
    only the last one carries padding (the spec's "chunk after encoding").
    """
    text = base64.b64encode(data)
    return [text[i : i + size] for i in range(0, len(text), size)] or [b""]


def icon_sequences(ident: str, cache_key: str, png: bytes) -> list[bytes]:
    """Sequences transmitting `png` as an icon (`p=icon`, `e=1`, `g=`).

    Chunks of <= 4096 base64 bytes; `d=0` on all but the last chunk. With a
    preceding `d=0` title chunk of the same `i=` this makes ONE notification
    with the icon; on its own (no title) it only primes the icon cache.
    """
    chunks = b64_chunks(png)
    out = []
    for index, chunk in enumerate(chunks):
        done = 1 if index == len(chunks) - 1 else 0
        out.append(osc(f"i={ident}:p=icon:e=1:g={cache_key}:d={done}", chunk))
    return out


# ---------------------------------------------------------------------------
# Generated icon: 64x64 RGBA PNG (stdlib zlib + struct only)
# ---------------------------------------------------------------------------


def _png_chunk(kind: bytes, data: bytes) -> bytes:
    crc = zlib.crc32(kind + data) & 0xFFFFFFFF
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", crc)


def make_icon_png(variant: int = 0, size: int = 64) -> bytes:
    """Return a colourful `size`x`size` RGBA PNG with transparent corners.

    The picture is a diagonal rainbow gradient on a rounded square, with a
    white ring and a centre dot. `variant` 0 and 1 differ (gradient direction,
    dot colour) so a cache-priming icon is distinguishable from the first.
    Scanlines use PNG filter 1 (Sub), which keeps the gradient small.
    """
    radius = size // 6
    centre = (size - 1) / 2.0
    rows = []
    for y in range(size):
        pixels = bytearray()
        for x in range(size):
            # Distance outside the rounded-square corners -> transparent.
            dx = max(radius - x, 0, x - (size - 1 - radius))
            dy = max(radius - y, 0, y - (size - 1 - radius))
            if dx * dx + dy * dy > radius * radius:
                pixels += bytes((0, 0, 0, 0))
                continue
            span = (x + y) if variant == 0 else (x + (size - 1 - y))
            hue = (span / (2.0 * size) + 0.33 * variant) % 1.0
            r, g, b = (int(c * 255) for c in colorsys.hsv_to_rgb(hue, 0.85, 0.95))
            dist = ((x - centre) ** 2 + (y - centre) ** 2) ** 0.5
            if size * 0.27 <= dist < size * 0.34:
                r, g, b = 255, 255, 255  # ring
            elif dist < size * 0.14:
                r, g, b = (30, 30, 30) if variant == 0 else (255, 255, 255)
            pixels += bytes((r, g, b, 255))
        # Filter type 1 (Sub): each byte minus the one a pixel (4 bytes) left.
        filtered = bytearray([1])
        for i, value in enumerate(pixels):
            left = pixels[i - 4] if i >= 4 else 0
            filtered.append((value - left) & 0xFF)
        rows.append(bytes(filtered))
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)  # 8-bit RGBA
    return (
        b"\x89PNG\r\n\x1a\n"
        + _png_chunk(b"IHDR", ihdr)
        + _png_chunk(b"IDAT", zlib.compress(b"".join(rows), 9))
        + _png_chunk(b"IEND", b"")
    )


ICON_PNG = make_icon_png(0)
ICON_PNG_2 = make_icon_png(1)


def check_png(data: bytes, size: int = 64) -> list[str]:
    """Return problems found in a PNG produced by `make_icon_png` (empty = ok)."""
    problems: list[str] = []
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        return ["bad PNG signature"]
    pos, idat, seen = 8, b"", []
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        kind = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        (crc,) = struct.unpack(">I", data[pos + 8 + length : pos + 12 + length])
        if crc != zlib.crc32(kind + body) & 0xFFFFFFFF:
            problems.append(f"bad CRC in {kind!r}")
        seen.append(kind)
        if kind == b"IHDR":
            if struct.unpack(">IIBBBBB", body) != (size, size, 8, 6, 0, 0, 0):
                problems.append("IHDR is not 64x64 8-bit RGBA")
        elif kind == b"IDAT":
            idat += body
        pos += 12 + length
    if seen != [b"IHDR", b"IDAT", b"IEND"]:
        problems.append(f"unexpected chunk order {seen!r}")
    raw = zlib.decompress(idat)
    if len(raw) != size * (1 + 4 * size):
        problems.append("decompressed size does not match 64x64 RGBA")
    elif raw[0] != 1 or raw[1:5] != bytes(4):
        problems.append("first scanline is not Sub-filtered transparent corner")
    return problems


# ---------------------------------------------------------------------------
# Reply parsing
# ---------------------------------------------------------------------------

_REPLY_RE = re.compile(rb"(?:\x1b\]|\x9d)99;(.*?)(?:\x1b\\|\x9c)", re.DOTALL)
_REPLY_START_RE = re.compile(rb"(?:\x1b\]|\x9d)99;")
_START_PREFIXES = (
    b"\x1b",
    b"\x1b]",
    b"\x1b]9",
    b"\x1b]99",
    b"\x9d",
    b"\x9d9",
    b"\x9d99",
)


@dataclass
class Reply:
    """One decoded OSC 99 reply written back by the terminal."""

    raw: bytes
    metadata: str
    payload: str

    def fields(self) -> dict[str, str]:
        """The metadata as a `{key: value}` dict."""
        return parse_meta(self.metadata)

    def describe(self) -> str:
        """A one-line human reading of what this reply means."""
        kind = self.fields().get("p")
        if kind == "close":
            if self.payload == "untracked":
                return "close report (untracked: the platform cannot observe the close)"
            return "close report"
        if kind == "alive":
            return f"alive list [{self.payload}]"
        if kind == "?":
            return "capability reply"
        if self.payload == "":
            return "activation report (whole notification)"
        if self.payload.isdigit():
            return f"activation report (button {self.payload}, 1-based)"
        return "unrecognised reply"


def parse_meta(meta: str) -> dict[str, str]:
    """Split `k=v:k=v` metadata into a dict (the first `=` separates)."""
    out: dict[str, str] = {}
    for item in meta.split(":"):
        if item:
            key, _, value = item.partition("=")
            out[key] = value
    return out


def split_replies(buf: bytes) -> tuple[list[Reply], bytes, bytes]:
    """Split `buf` into `(replies, other_bytes, incomplete_tail)`.

    A reply is `ESC ] 99 ; meta ; payload ST` (or the 8-bit forms, `0x9d` ...
    `0x9c`). `incomplete_tail` is an unterminated reply (or the start of one)
    to carry over to the next read; `other_bytes` is everything else.
    """
    replies: list[Reply] = []
    other = bytearray()
    pos = 0
    for match in _REPLY_RE.finditer(buf):
        other += buf[pos : match.start()]
        body = match.group(1)
        meta, _, payload = body.partition(b";")
        replies.append(
            Reply(
                raw=match.group(0),
                metadata=meta.decode("utf-8", "replace"),
                payload=payload.decode("utf-8", "replace"),
            )
        )
        pos = match.end()
    rest = buf[pos:]
    tail = b""
    start = _REPLY_START_RE.search(rest)
    if start:
        tail = rest[start.start() :]
        rest = rest[: start.start()]
    else:
        for prefix in sorted(_START_PREFIXES, key=len, reverse=True):
            if rest.endswith(prefix):
                tail = prefix
                rest = rest[: len(rest) - len(prefix)]
                break
    other += rest
    return replies, bytes(other), tail


def print_reply(label: str, reply: Reply) -> None:
    """Print one reply on its own line, decoded."""
    print(f"    {label}: {escaped(reply.raw)}")
    print(
        f"        => {reply.describe()}; metadata {reply.fields()}; payload {reply.payload!r}"
    )


StopWhen = Callable[[list[Reply], bytes], bool]


def at_least(count: int) -> StopWhen:
    """A `stop_when` that stops reading once `count` replies have arrived."""
    return lambda replies, _other: len(replies) >= count


# ---------------------------------------------------------------------------
# tty handling (POSIX only)
# ---------------------------------------------------------------------------


def tty_available() -> bool:
    """True when replies can be read: POSIX termios and an interactive stdin."""
    return _POSIX_TTY and sys.stdin.isatty()


@contextlib.contextmanager
def quiet_tty() -> Iterator[int | None]:
    """Put stdin into no-echo, non-canonical mode; restore on exit.

    Yields the stdin fd, or None when replies cannot be read. Signals stay
    enabled (Ctrl-C still works) and input translation is off, so replies
    arrive byte-exact (including 8-bit C1 forms). `TCSANOW` is used so that
    bytes already queued are NOT flushed away.
    """
    if not tty_available():
        yield None
        return
    fd = sys.stdin.fileno()
    saved = termios.tcgetattr(fd)
    new = termios.tcgetattr(fd)
    new[0] &= ~(
        termios.BRKINT
        | termios.ICRNL
        | termios.INPCK
        | termios.ISTRIP
        | termios.IXON
        | termios.INLCR
        | termios.IGNCR
    )
    new[3] &= ~(termios.ECHO | termios.ICANON | termios.IEXTEN)
    new[6][termios.VMIN] = 1
    new[6][termios.VTIME] = 0
    termios.tcsetattr(fd, termios.TCSANOW, new)
    try:
        yield fd
    finally:
        termios.tcsetattr(fd, termios.TCSADRAIN, saved)


def _read_some(fd: int, timeout: float) -> bytes | None:
    """Wait up to `timeout` s for input; return what is available or None."""
    ready, _, _ = select.select([fd], [], [], max(timeout, 0.0))
    if not ready:
        return None
    return os.read(fd, 4096)


def _drain(fd: int, grace: float = 0.0) -> bytes:
    """Read every byte pending on `fd` without blocking (plus `grace` seconds)."""
    data = bytearray()
    wait = grace
    while True:
        chunk = _read_some(fd, wait)
        if not chunk:
            break
        data += chunk
        wait = 0.0
    return bytes(data)


def print_late(data: bytes) -> None:
    """Print stray bytes as `late reply:` (OSC 99) / `late input:` (anything else)."""
    if not data:
        return
    replies, other, tail = split_replies(data)
    for reply in replies:
        print_reply("late reply", reply)
    leftover = other + tail
    if leftover.strip():
        print(f"    late input (ignored): {escaped(leftover)}")


def drain_stray(grace: float = 0.0) -> None:
    """Drain and print stray pending stdin bytes (call before any prompt)."""
    if OPTS.dry_run:
        return
    with quiet_tty() as fd:
        if fd is not None:
            print_late(_drain(fd, grace))


def write_sequence(sequence: bytes) -> None:
    """Show and write one sequence to the terminal (or just show it in dry-run)."""
    print(f"    sending: {escaped(sequence)}")
    if OPTS.dry_run:
        OPTS.sent.append(sequence)
        return
    sys.stdout.flush()
    sys.stdout.buffer.write(sequence)
    sys.stdout.buffer.flush()
    OPTS.sent.append(sequence)


def emit(metadata: str, payload: bytes | str = b"") -> None:
    """Send one OSC 99 sequence with no reply expected."""
    write_sequence(osc(metadata, payload))


def emit_all(sequences: Iterable[bytes]) -> None:
    """Send several prepared sequences, no reply expected."""
    for sequence in sequences:
        write_sequence(sequence)


def send_and_read(
    sequences: Iterable[bytes],
    timeout: float = 3.0,
    stop_when: StopWhen | None = None,
) -> list[Reply]:
    """No-echo mode -> drain stray bytes -> emit `sequences` -> read replies.

    Reading ends after `timeout` seconds, or as soon as `stop_when(replies,
    other_bytes)` returns true. Each reply is printed on its own line, decoded.
    Returns the replies. In dry-run mode the sequences are only printed.
    """
    sequences = list(sequences)
    if OPTS.dry_run:
        emit_all(sequences)
        print("    (dry-run: not reading replies)")
        return []
    with quiet_tty() as fd:
        if fd is None:
            emit_all(sequences)
            print("    (reply reading needs a POSIX tty; skipping on this platform)")
            return []
        print_late(_drain(fd))
        emit_all(sequences)
        replies: list[Reply] = []
        other = bytearray()
        buf = b""
        deadline = time.monotonic() + timeout
        while True:
            chunk = _read_some(fd, deadline - time.monotonic())
            if not chunk:
                break
            got, rest, buf = split_replies(buf + chunk)
            other += rest
            for reply in got:
                replies.append(reply)
                print_reply(f"reply {len(replies)}", reply)
            if stop_when is not None and stop_when(replies, bytes(other)):
                break
    if not replies:
        print(f"    no reply within {timeout:g} s")
    if other:
        print(f"    other input (not OSC 99): {escaped(bytes(other))}")
    return replies


def hold(seconds: float) -> None:
    """Wait `seconds` with no echo, collecting stray bytes; print them afterwards."""
    if OPTS.dry_run:
        return
    with quiet_tty() as fd:
        if fd is None:
            time.sleep(seconds)
            return
        data = bytearray()
        deadline = time.monotonic() + seconds
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            chunk = _read_some(fd, remaining)
            if chunk:
                data += chunk
    print_late(bytes(data))


def countdown(seconds: int, action: str) -> None:
    """Show a visible countdown, then return (the caller sends afterwards).

    Never asks for a keypress: pressing Enter in the window would refocus it
    and defeat steps that need freminal unfocused or in a background tab.
    """
    if OPTS.dry_run:
        print(f"    (dry-run: {seconds} s countdown skipped) {action}")
        return
    stray = bytearray()
    for remaining in range(seconds, 0, -1):
        print(f"\r    {action} in {remaining} s ... ", end="", flush=True)
        with quiet_tty() as fd:
            if fd is None:
                time.sleep(1.0)
            else:
                deadline = time.monotonic() + 1.0
                while time.monotonic() < deadline:
                    chunk = _read_some(fd, deadline - time.monotonic())
                    if chunk:
                        stray += chunk
    print(f"\r    {action} now.{' ' * 20}")
    print_late(bytes(stray))


def pause(prompt: str = "Press Enter to continue...") -> None:
    """Wait for Enter with no echo; late replies that arrive meanwhile are shown.

    Only for steps where the freminal window stays focused: the Enter keypress
    itself requires the window to have focus.
    """
    print(f"    {prompt}", end="", flush=True)
    if OPTS.dry_run:
        print(" (dry-run: skipped)")
        return
    with quiet_tty() as fd:
        if fd is None:
            try:
                input()
            except EOFError:
                pass
            return
        buf = b""
        while True:
            data = os.read(fd, 4096)
            if not data:
                break
            got, other, buf = split_replies(buf + data)
            if got:
                print()
                for reply in got:
                    print_reply("late reply", reply)
                print(f"    {prompt}", end="", flush=True)
            if b"\r" in other or b"\n" in other:
                break
    print()


def read_line(prompt: str) -> str | None:
    """Prompt for one line with no tty echo; replies arriving meanwhile are shown.

    Replaces `input()` for the menu: a reply that arrives while the prompt is
    waiting (say, a click on an old notification) is parsed out and printed as
    "late reply:" instead of being echoed into the typed text. Returns None on
    EOF (Ctrl-D on an empty line).
    """
    if OPTS.dry_run or not tty_available():
        try:
            return input(prompt)
        except EOFError:
            return None
    print(prompt, end="", flush=True)
    typed = ""
    buf = b""
    with quiet_tty() as fd:
        if fd is None:  # pragma: no cover - tty_available() was true above
            return None
        while True:
            data = os.read(fd, 4096)
            if not data:
                return None
            got, other, buf = split_replies(buf + data)
            if got:
                print()
                for reply in got:
                    print_reply("late reply", reply)
                print(f"{prompt}{typed}", end="", flush=True)
            for byte in other:
                if byte in (0x0D, 0x0A):
                    print()
                    return typed
                if byte == 0x04 and not typed:
                    print()
                    return None
                if byte in (0x7F, 0x08):
                    if typed:
                        typed = typed[:-1]
                        print("\b \b", end="", flush=True)
                elif 0x20 <= byte < 0x7F:
                    typed += chr(byte)
                    print(chr(byte), end="", flush=True)


# ---------------------------------------------------------------------------
# Step plumbing
# ---------------------------------------------------------------------------


def expect(*lines: str) -> None:
    """Print one or more `Expect:` lines (continuations are indented)."""
    for index, line in enumerate(lines):
        label = "Expect:" if index == 0 else "       "
        print(f"    {label} {line}")


def note(text: str) -> None:
    """Print an instruction or aside for the human."""
    print(f"    Note: {text}")


def delay(seconds: float) -> None:
    """Short quiet pause between notifications (keeps toasts under the cap of 5)."""
    hold(seconds)


# ---------------------------------------------------------------------------
# Steps: content (title, body, chunking, encoding, limits)
# ---------------------------------------------------------------------------


def step_minimal_title() -> None:
    """Empty metadata, plain-text title: `ESC ] 99 ; ; Hello world ST`."""
    expect(
        "one notification titled 'Hello world', no body.",
        "Toast: an info toast that closes itself after its default ~6 s.",
        "No reply of any kind comes back (no a=report / c=1 was requested).",
    )
    emit("", b"Hello world")


def step_title_body_chunked() -> None:
    """Title chunk (d=0) then body chunk (d=1) sharing i=n1."""
    expect(
        "after the first chunk NOTHING is shown (d=0 holds the display);",
        "after the second chunk ONE notification appears: title 'My Title',",
        "body 'My body text'. (A body-only notification would use the body as title.)",
    )
    emit("i=n1:p=title:d=0", b"My Title")
    delay(2.0)
    emit("i=n1:p=body:d=1", b"My body text")


def step_chunked_title() -> None:
    """Three title chunks sharing i=c1; only the d=1 chunk finalises."""
    expect(
        "nothing shown until the last chunk, then ONE notification titled",
        "'Part one part two part three' (same-typed payloads are concatenated).",
    )
    emit("i=c1:p=title:d=0", b"Part one ")
    emit("i=c1:p=title:d=0", b"part two ")
    emit("i=c1:p=title:d=1", b"part three")


def step_idless_chunked() -> None:
    """A chunked notification with NO i=: d=0 chunk, then the final chunk."""
    expect(
        "chunks with no i= share one 'unidentified' slot, so they still reassemble:",
        "ONE notification titled 'Id-less chunked title' after the final chunk.",
        "Being id-less it can never be updated, closed or listed by p=alive.",
    )
    emit("p=title:d=0", b"Id-less ")
    emit("p=title:d=1", b"chunked title")


def step_base64_body() -> None:
    """Body sent base64 (e=1) with non-ASCII text; title chunk precedes it."""
    body = "Base64 body \u2713 h\u00e9llo \u65e5\u672c\u8a9e"
    expect(
        f"title 'Base64 test', body {body!r} (UTF-8 survives the base64 round trip).",
    )
    emit("i=b1:p=title:d=0", b"Base64 test")
    emit("i=b1:p=body:e=1:d=1", b64(body))


LONG_BODY = "".join(f"[{n:04d}]" for n in range(1, 400))[:2000]


def step_long_body() -> None:
    """A 2000-character body (one 2000-byte chunk): freminal cuts it at 1024."""
    kept = LONG_BODY[:1024]
    expect(
        "the body is cut to its first 1024 characters with NO ellipsis marker",
        f"(kitty's limit_size): it ends '...{kept[-14:]}' and never reaches [0334].",
        "The toast may additionally clip or wrap what it shows; the desktop",
        "daemon receives the 1024-character body.",
    )
    emit("i=long1:p=title:d=0", b"Long body (2000 chars sent)")
    emit("i=long1:p=body:d=1", LONG_BODY)


# ---------------------------------------------------------------------------
# Steps: lifecycle (update, close, activation, buttons, focus)
# ---------------------------------------------------------------------------


def step_update_in_place() -> None:
    """Send i=u1 with c=1, then i=u1 again while the first is still visible."""
    expect(
        "first: 'Version one' stays up (w=0). Second send: the SAME notification is",
        "replaced in place (toast keyed replace / D-Bus replaces_id) -- not a second",
        "one. The replaced notification sends NO close report, and the update did",
        "not ask for one, so NO report arrives at all (checked below for 3 s).",
        "macOS/Windows: the system leg cannot update, so it shows another one.",
    )
    send_and_read([osc("i=u1:c=1:w=0", "Version one (c=1, stays up)")], timeout=0.5)
    pause("Look at it, then press Enter to send the update (window stays focused)...")
    send_and_read([osc("i=u1:w=5000", "Version two (replaced in place)")], timeout=3.0)
    note(
        "'Version two' expires by itself after 5 s (w=5000); it asked for no close report."
    )


def step_p_close() -> None:
    """Show i=pc1 (c=1, w=0), then close it programmatically with p=close."""
    expect(
        "'Close me' stays visible (w=0) during the countdown, then disappears when",
        "the p=close request arrives (toast fades out over ~400 ms), and ONE close",
        "report arrives:  ESC ] 99 ; i=pc1:p=close ; ST",
        "macOS/Windows: the system notification cannot be closed; with `both`",
        "routing the toast closes. `p=close;untracked` arrived at display instead.",
    )
    send_and_read([osc("i=pc1:c=1:w=0", "Close me (p=close follows)")], timeout=1.0)
    countdown(4, "sending p=close")
    send_and_read([osc("i=pc1:p=close", "")], timeout=5.0, stop_when=at_least(1))


def step_activation_and_close() -> None:
    """i=ac1 with a=report and c=1: click it, then expect two reports."""
    expect(
        "click the toast body (or the desktop notification). Two reports arrive,",
        "in this order:",
        "  1. activation: ESC ] 99 ; i=ac1 ; ST        (empty payload = whole notification)",
        "  2. close:      ESC ] 99 ; i=ac1:p=close ; ST (c=1: once every leg has closed)",
        "(a=report adds `report` to the default `focus`, so the click also raises",
        "freminal.) macOS/Windows: only `p=close;untracked` arrives, at display.",
    )
    note("click it within 25 s; the notification stays up until clicked (w=0).")
    send_and_read(
        [osc("i=ac1:a=report:c=1:w=0", "Click me (activation + close)")],
        timeout=25.0,
        stop_when=at_least(2),
    )


def step_buttons() -> None:
    """Title + p=buttons Yes/No/Maybe with a=report and c=1."""
    expect(
        "a notification with three buttons: Yes / No / Maybe. Clicking button 2:",
        "  1. ESC ] 99 ; i=btn1 ; 2 ST         (1-based button number)",
        "  2. ESC ] 99 ; i=btn1:p=close ; ST   (c=1)",
        "Toast buttons are clickable in the freminal window itself; the desktop",
        "notification shows them only if the daemon has the `actions` capability.",
        "Clicking the body instead gives an empty activation payload.",
    )
    note("click a button within 25 s (the notification stays up until then).")
    send_and_read(
        [
            osc("i=btn1:a=report:c=1:w=0:p=title:d=0", "Pick one"),
            osc("i=btn1:p=buttons", f"Yes{U2028}No{U2028}Maybe"),
        ],
        timeout=25.0,
        stop_when=at_least(2),
    )


def step_focus_action() -> None:
    """a=focus (the default) switches back to the source pane; a=-focus does not."""
    expect(
        "part 1 (default actions include focus): after the countdown a toast and a",
        "desktop notification appear. Click it: freminal switches back to the tab",
        "and pane that sent it (unzooming if needed) and raises the window.",
        "part 2 (a=-focus): clicking it does NOT move you -- the active tab/pane",
        "stays where it is. (A compositor may refuse the raise on some setups.)",
    )
    print("    PART 1: switch to ANOTHER TAB (and/or another pane) now.")
    countdown(8, "sending i=f1")
    emit("i=f1:w=0", "Part 1: click me -> should focus the originating pane")
    pause("Press Enter (after clicking it you are back here) for part 2...")
    print("    PART 2: switch to ANOTHER TAB (and/or pane) again.")
    countdown(8, "sending i=f2:a=-focus")
    emit("i=f2:a=-focus:w=0", "Part 2: click me -> must NOT move focus")
    note("dismiss the leftover notifications (w=0 keeps them up).")


# ---------------------------------------------------------------------------
# Steps: urgency, sound, icons, naming, expiry
# ---------------------------------------------------------------------------


def step_urgency() -> None:
    """u=0 / u=1 / u=2, one second apart."""
    expect(
        "toast: low (u=0) is an info toast lasting ~3 s; normal (u=1) an info toast",
        "lasting ~6 s; critical (u=2) an ERROR-styled toast that stays until dismissed.",
        "Desktop (Linux/BSD): the daemon receives the `urgency` hint 0/1/2.",
        "macOS/Windows: no urgency on the system leg.",
    )
    emit("i=urg-low:u=0", "Urgency: low (u=0)")
    delay(1.0)
    emit("i=urg-normal:u=1", "Urgency: normal (u=1)")
    delay(1.0)
    emit("i=urg-critical:u=2", "Urgency: critical (u=2) -- dismiss me")


def step_sounds() -> None:
    """s=system, s=silent and a standard name (warn)."""
    expect(
        "the toast never plays sound. Desktop (Linux/BSD): `system` sends no sound",
        "hint (daemon default), `silent` sends suppress-sound, `warn` sends",
        "sound-name=dialog-warning -- but only a daemon with the `sound` capability",
        "plays it, and many daemons ignore the hints entirely. A daemon without",
        "`sound` (check: busctl --user call org.freedesktop.Notifications",
        "/org/freedesktop/Notifications org.freedesktop.Notifications GetCapabilities)",
        "plays NOTHING for any of the three; that is correct, not a freminal bug.",
        "macOS/Windows: `silent` is silent; the others play the platform default.",
    )
    emit(f"i=snd-system:s={b64('system')}", "Sound: system")
    delay(1.5)
    emit(f"i=snd-silent:s={b64('silent')}", "Sound: silent")
    delay(1.5)
    emit(f"i=snd-warn:s={b64('warn')}", "Sound: warn (dialog-warning)")


STANDARD_ICONS = (
    "error",
    "warn",
    "info",
    "question",
    "help",
    "file-manager",
    "system-monitor",
    "text-editor",
)


def step_icon_names() -> None:
    """Each of the eight standard icon names (n=), 1.5 s apart."""
    expect(
        "eight notifications, one per standard name. Toast: the matching bundled-",
        "font glyph in the icon slot (the accent colour still follows urgency).",
        "Desktop (Linux/BSD): the matching freedesktop icon, sent as both app_icon",
        "and the image-path hint (error=dialog-error,",
        "warn=dialog-warning, info=dialog-information, question=dialog-question,",
        "help=system-help, file-manager=system-file-manager,",
        "system-monitor=utilities-system-monitor, text-editor=utilities-text-editor).",
        "macOS/Windows: a drawn glyph image of the same name.",
        "The last one uses a NON-standard name: the toast ignores it; Linux/BSD",
        "shows it only if the icon theme scan finds it.",
    )
    for name in STANDARD_ICONS:
        emit(f"i=icon-{name}:n={b64(name)}", f"Icon by name: {name}")
        delay(1.5)
    emit(f"i=icon-custom:n={b64('utilities-terminal')}", "Icon by non-standard name")


def step_icon_data_cache_prime() -> None:
    """Transmitted 64x64 PNG + g= cache, cache reuse, then icon-only priming."""
    expect(
        "A) 'Icon by data': a notification with the transmitted rainbow icon,",
        "   cached under g=cachekey1. Shown on the DESKTOP leg (Linux/BSD, macOS,",
        "   Windows) only. The TOAST shows NO icon here: toasts never render",
        "   transmitted image data, and no standard n= name was sent.",
        "B) 'Icon from cache': sent with g=cachekey1 and NO icon bytes -> same icon.",
        "C) priming: `i=prime1:p=icon:e=1:g=cachekey2` with no title shows NOTHING and",
        "   sends no reply (it only fills the cache). D) 'Primed icon' with only",
        "   g=cachekey2 then shows the OTHER icon (reversed gradient, white centre).",
    )
    print(
        f"    (icon: {len(ICON_PNG)} byte 64x64 RGBA PNG, sent as <=4096-byte chunks)"
    )
    emit("i=icon-data1:p=title:d=0", "Icon by data")
    emit_all(icon_sequences("icon-data1", "cachekey1", ICON_PNG))
    delay(2.0)
    emit("i=icon-data2:g=cachekey1", "Icon from cache (no icon bytes sent)")
    delay(2.0)
    print("    Priming the cache (nothing should appear):")
    send_and_read(icon_sequences("prime1", "cachekey2", ICON_PNG_2), timeout=1.5)
    emit("i=prime1b:g=cachekey2", "Primed icon (cachekey2)")


def step_app_name_and_category() -> None:
    """f= application name and t= notification type (category)."""
    expect(
        "a notification from application 'My Test App' of type 'im.received'.",
        "Desktop (Linux/BSD): D-Bus app_name = 'My Test App' (your daemon may group",
        "or label by it) and the `category` hint = 'im.received'.",
        "Both values are also what osc99_filters rules `app:` and `type:` match --",
        "see the filters step.",
    )
    emit(
        f"i=app1:f={b64('My Test App')}:t={b64('im.received')}",
        "App name + category test",
    )


def step_expiry() -> None:
    """w=3000, w=0 and w=-1."""
    expect(
        "w=3000 (c=1): freminal closes EVERY leg at 3 s even if you hover it, then",
        "sends the close report  ESC ] 99 ; i=exp3 : p=close ; ST  (read below).",
        "w=0: stays until dismissed (toast: until you close it).",
        "w=-1: OS/default lifetime (toast: its ~6 s default for normal urgency).",
        "macOS/Windows: w= goes to the OS and freminal does not enforce it.",
    )
    emit("i=exp-never:w=0", "Never expires (w=0) -- dismiss me")
    emit("i=exp-default:w=-1", "Default expiry (w=-1)")
    note("hover the next one; it must still close at 3 s.")
    send_and_read(
        [osc("i=exp3:w=3000:c=1", "Expires in 3000 ms (hover me)")],
        timeout=7.0,
        stop_when=at_least(1),
    )


# ---------------------------------------------------------------------------
# Steps: occasions
# ---------------------------------------------------------------------------


def step_occasion_unfocused() -> None:
    """o=unfocused / o=invisible while the window is focused, then unfocused."""
    expect(
        "While this pane is focused: 'SHOULD NOT SHOW' (o=unfocused) and (o=invisible)",
        "appear NOWHERE. o=always shows.",
        "After the countdown (you have clicked another window): the o=unfocused AND",
        "o=invisible notifications BOTH show -- 'visible' requires OS-window focus,",
        "so an unfocused window counts as not visible even when fully on screen.",
        "With the default routing_osc99 = 'both', each shown notification ALSO",
        "appears as a toast in the freminal window (routing does not depend on",
        "focus; only `system_when_unfocused` does). These use w=20000 so the toasts",
        "are still there when you come back to look.",
        "(Wayland compositors do not report minimisation: minimised = restored.)",
    )
    emit("i=occ-f-unf:o=unfocused", "SHOULD NOT SHOW (focused, o=unfocused)")
    emit("i=occ-f-inv:o=invisible", "SHOULD NOT SHOW (focused, o=invisible)")
    emit("i=occ-f-alw:o=always:w=20000", "Shows: o=always while focused")
    print(
        "    NOW unfocus freminal: click another app or minimise it. Do NOT press Enter."
    )
    countdown(6, "sending the unfocused/invisible set")
    emit("i=occ-unf:o=unfocused:w=20000", "Shows: o=unfocused (window unfocused)")
    emit(
        "i=occ-inv:o=invisible:w=20000",
        "Shows: o=invisible (window unfocused counts as hidden)",
    )
    emit("i=occ-alw:o=always:w=20000", "Shows: o=always")


def step_occasion_background_tab() -> None:
    """The pane lives in a background tab: o=unfocused and o=invisible both show."""
    expect(
        "the window stays focused but this pane's tab is inactive, so the pane is",
        "neither focused nor visible: BOTH the o=unfocused and the o=invisible",
        "notifications show. (Switch back to this tab and re-run to see them",
        "suppressed.)",
    )
    print("    Open ANOTHER TAB and switch to it (keep the freminal window focused).")
    print("    This script keeps running in the background tab. Do NOT come back yet.")
    countdown(8, "sending from the background tab")
    emit("i=bg-unf:o=unfocused:w=20000", "Background tab: o=unfocused shows")
    emit("i=bg-inv:o=invisible:w=20000", "Background tab: o=invisible shows")
    note("switch back to the original tab to continue.")


# ---------------------------------------------------------------------------
# Steps: p=alive, p=?, filters, controls
# ---------------------------------------------------------------------------


def step_alive() -> None:
    """Two live notifications, then p=alive; then p=alive with no id."""
    expect(
        "reply:  ESC ] 99 ; i=alq : p=alive ; al1,al2 ST  (sorted; the i= echoes the",
        "request; any OTHER still-open identified notification of THIS pane, e.g.",
        "from earlier w=0 steps, is listed too).",
        "p=alive with no i= is a no-op: NO reply (checked for 2 s).",
        "After p=close on both, a new poll no longer lists al1/al2.",
        "macOS/Windows with system routing alone: always an empty list (untracked).",
    )
    emit("i=al1:w=0", "Alive 1 (w=0)")
    emit("i=al2:w=0", "Alive 2 (w=0)")
    delay(1.0)
    print("    -- poll with an id:")
    send_and_read([osc("i=alq:p=alive", "")], timeout=3.0, stop_when=at_least(1))
    print("    -- poll with NO id (expect silence):")
    send_and_read([osc("p=alive", "")], timeout=2.0)
    print("    -- close both, then poll again:")
    send_and_read(
        [osc("i=al1:p=close", ""), osc("i=al2:p=close", ""), osc("i=alq2:p=alive", "")],
        timeout=3.0,
        stop_when=at_least(1),
    )


def step_alive_other_pane_open() -> None:
    """Pane A half of the isolation demo: leave one long-lived notification."""
    expect(
        "one notification 'Opened by pane A' (w=0) stays up. Now SPLIT or open another",
        "pane and run the NEXT step there: its poll must NOT list 'paneA-open'.",
    )
    emit("i=paneA-open:w=0", "Opened by pane A (leave open)")


def step_alive_other_pane_poll() -> None:
    """Pane B half: poll p=alive from a different pane than the one that opened."""
    expect(
        "reply:  ESC ] 99 ; i=poll-b : p=alive ; ST  with an EMPTY list (or only ids",
        "opened by THIS pane): p=alive, update and p=close are scoped to the",
        "requesting pane, so 'paneA-open' from the other pane is never listed.",
    )
    send_and_read([osc("i=poll-b:p=alive", "")], timeout=3.0, stop_when=at_least(1))


# Reference replies copied from Documents/KITTY_PROTOCOL_REFERENCE.md
# ("Capability handshake"); written without spaces around the colons.
CAP_REFERENCE = {
    "both or system, Linux/BSD, daemon with body+actions+sound": (
        "a=focus,report:o=always,unfocused,invisible:u=0,1,2:"
        "p=title,body,?,close,icon,alive,buttons:c=1:w=1:"
        "s=system,silent,error,info,question,warn,warning"
    ),
    "toast routing, any platform": (
        "a=focus,report:o=always,unfocused,invisible:u=0,1,2:"
        "p=title,body,?,close,alive,buttons:c=1:w=1:s=system,silent"
    ),
    "system, macOS or Windows": (
        "o=always,unfocused,invisible:p=title,body,?,close,icon,alive:s=system,silent"
    ),
    "both, macOS": (
        "a=focus,report:o=always,unfocused,invisible:u=0,1,2:"
        "p=title,body,?,close,icon,alive,buttons:s=system,silent"
    ),
}


def step_capability_handshake() -> None:
    """p=? followed by a DA1 request; the OSC reply must come first."""
    expect(
        "an OSC 99 reply  ESC ] 99 ; i=q1 : p=? ; <caps> ST  arrives BEFORE the DA1",
        "answer (ESC [ ? ... c), because the PTY thread answers in stream order.",
        "Keys appear in the order a,o,u,p,c,w,s. For the default `both` routing on",
        "Linux/BSD with a full daemon expect the first reference below (the `s`",
        "list lacks the standard names when the daemon has no `sound` capability).",
        "With routing_osc99 = 'toast' expect the second. If only the DA1 answer",
        "arrives, OSC 99 is disabled (enabled/osc_99/routing) -- as for any app.",
    )
    replies = send_and_read(
        [osc("i=q1:p=?", ""), b"\x1b[c"],
        timeout=3.0,
        stop_when=lambda got, other: bool(got) and other.endswith(b"c"),
    )
    if not replies:
        return
    got = parse_meta(replies[0].payload)
    print("    capabilities, key by key:")
    for key, value in got.items():
        print(f"      {key} = {value}")
    for name, reference in CAP_REFERENCE.items():
        want = parse_meta(reference)
        if want == got:
            print(f"    MATCHES reference: {name}")
            continue
        diffs = [
            f"{key}: got {got.get(key)!r}, reference {want.get(key)!r}"
            for key in sorted(set(want) | set(got))
            if want.get(key) != got.get(key)
        ]
        print(f"    differs from '{name}':")
        for line in diffs:
            print(f"        {line}")
    print("    reference strings:")
    for name, reference in CAP_REFERENCE.items():
        print(f"      [{name}]\n        {reference}")


def step_filters() -> None:
    """osc99_filters = ["title:^FILTERED"]: a matching notification is dropped silently."""
    expect(
        "with the filter configured and reloaded: the 'FILTERED ...' notification is",
        "NOT shown, NOT registered and gets NO reply (even though it asked for a=report",
        "and c=1 -- checked for 3 s); the normal one IS shown.",
        "Without the filter both show, which tells you the config was not applied.",
        'Other rules: "app:My Test App" and "type:im" match the f= / t= values of',
        'the app-name step; `all` matches everything; case-insensitive: "title:(?i)^filtered".',
    )
    print("    Add this to your config.toml, then reload (hot-reload or restart):")
    print("        [notifications]")
    print('        osc99_filters = ["title:^FILTERED"]')
    pause("Press Enter once the filter is active...")
    send_and_read(
        [osc("i=flt1:a=report:c=1:w=0", "FILTERED: this must not appear")],
        timeout=3.0,
    )
    emit("i=flt2", "Normal notification (not filtered)")


def step_controls_immediate() -> None:
    """A control (p=?, d=0) mid-way through a chunked notification is answered at once."""
    expect(
        "the p=? reply arrives immediately, while the chunked notification is still",
        "incomplete (controls act at once whatever d= says, and do NOT discard the",
        "pending transfer -- a documented difference from kitty). Then the transfer",
        "completes and ONE notification shows: 'Control mid-chunk' / 'Chunk finished'.",
    )
    emit("i=ctl1:p=title:d=0", b"Control mid-chunk")
    replies = send_and_read(
        [osc("i=x:p=?:d=0", "")], timeout=3.0, stop_when=at_least(1)
    )
    if replies:
        print("    reply came before the notification finished: nothing is shown yet.")
    emit("i=ctl1:p=body:d=1", b"Chunk finished")


STEPS: list[tuple[str, Callable[[], None]]] = [
    ("Minimal title", step_minimal_title),
    ("Title + body, chunked by id", step_title_body_chunked),
    ("Chunked title (3 chunks)", step_chunked_title),
    ("Id-less chunked notification", step_idless_chunked),
    ("Base64 body with non-ASCII (e=1)", step_base64_body),
    ("Long body (2000 chars -> 1024)", step_long_body),
    ("Update in place (no close report)", step_update_in_place),
    ("p=close (programmatic close + report)", step_p_close),
    ("Activation + close reports (a=report, c=1)", step_activation_and_close),
    ("Buttons (p=buttons, a=report)", step_buttons),
    ("Focus action (a=focus / a=-focus)", step_focus_action),
    ("Urgency (u=0/1/2)", step_urgency),
    ("Sounds (s=system / silent / warn)", step_sounds),
    ("Icons by standard name (n=)", step_icon_names),
    ("Icon data, g= cache reuse, icon-only priming", step_icon_data_cache_prime),
    ("App name (f=) and category (t=)", step_app_name_and_category),
    ("Expiry (w=3000 / 0 / -1)", step_expiry),
    ("Occasion: window unfocused (o=)", step_occasion_unfocused),
    ("Occasion: background tab (o=)", step_occasion_background_tab),
    ("p=alive (listing, no-id poll, after close)", step_alive),
    ("p=alive isolation, pane A: leave one open", step_alive_other_pane_open),
    ("p=alive isolation, pane B: poll", step_alive_other_pane_poll),
    ("Capability handshake (p=?) + DA1 ordering", step_capability_handshake),
    ("Filters (osc99_filters)", step_filters),
    ("Controls act immediately (p=? with d=0)", step_controls_immediate),
]


def run_step(number: int) -> None:
    """Print the header for step `number` (1-based) and run it."""
    title, fn = STEPS[number - 1]
    print()
    print(f"=== Step {number}: {title} ===")
    doc = inspect.getdoc(fn) or ""
    if doc:
        print(f"    {doc}")
    fn()


# ---------------------------------------------------------------------------
# Dry-run validation
# ---------------------------------------------------------------------------

_VALUE_RE = re.compile(r"^[A-Za-z0-9\-_/+.,(){}\[\]*&^%$#@!`~?=]*$")
_ID_RE = re.compile(r"^[A-Za-z0-9_+.\-]+$")
_KNOWN_KEYS = set("acdefginopstuw")
_PAYLOAD_TYPES = {"title", "body", "close", "icon", "alive", "buttons", "?"}
_CONTENT_TYPES = {"title", "body", "icon", "buttons"}


def _bad_plain_text(text: str) -> bool:
    return any(ord(c) < 0x20 or 0x7F <= ord(c) <= 0x9F for c in text)


def validate_sequence(sequence: bytes) -> list[str]:
    """Return the problems found in one emitted sequence (empty = well-formed)."""
    if not sequence.startswith(b"\x1b]99;"):
        return [] if sequence == b"\x1b[c" else [f"not OSC 99: {escaped(sequence)}"]
    problems: list[str] = []
    if not sequence.endswith(ST):
        return ["missing ST terminator"]
    inner = sequence[5:-2]
    if b"\x1b" in inner or b"\x07" in inner:
        problems.append("ESC/BEL inside the body")
    meta_b, sep, payload = inner.partition(b";")
    if not sep:
        return problems + ["missing the second semicolon"]
    try:
        meta = meta_b.decode("ascii")
    except UnicodeDecodeError:
        return problems + ["metadata is not ASCII"]
    fields: dict[str, str] = {}
    for item in meta.split(":") if meta else []:
        key, eq, value = item.partition("=")
        if not eq or len(key) != 1 or not key.isalpha():
            problems.append(f"bad metadata item {item!r}")
            continue
        if key not in _KNOWN_KEYS:
            problems.append(f"unknown key {key!r}")
        if not _VALUE_RE.match(value):
            problems.append(f"bad value characters in {item!r}")
        fields[key] = value
    for key in ("i", "g"):
        if key in fields and not _ID_RE.match(fields[key]):
            problems.append(f"{key}= is not a valid identifier")
    for key, allowed in (
        ("d", {"0", "1"}),
        ("e", {"0", "1"}),
        ("c", {"0", "1"}),
        ("u", {"0", "1", "2"}),
        ("o", {"always", "unfocused", "invisible"}),
    ):
        if key in fields and fields[key] not in allowed:
            problems.append(f"{key}={fields[key]!r} not in {sorted(allowed)}")
    ptype = fields.get("p", "title")
    if ptype not in _PAYLOAD_TYPES:
        problems.append(f"unknown p= value {ptype!r}")
    if "w" in fields and not re.fullmatch(r"-1|\d+", fields["w"]):
        problems.append(f"w={fields['w']!r} is not an integer >= -1")
    for word in fields.get("a", "").split(","):
        if word and word.lstrip("-") not in ("report", "focus"):
            problems.append(f"a= word {word!r}")
    for key in "fnst":
        if key in fields:
            try:
                text = base64.b64decode(fields[key], validate=True).decode("utf-8")
                if not text or len(text) > 256 or _bad_plain_text(text):
                    problems.append(f"{key}= decodes to unusable text")
            except ValueError:
                problems.append(f"{key}= is not base64 UTF-8")
    encoded = fields.get("e") == "1"
    if encoded:
        if len(payload) > MAX_B64_CHUNK:
            problems.append(f"base64 chunk is {len(payload)} bytes (> {MAX_B64_CHUNK})")
        try:
            base64.b64decode(payload, validate=True)
        except ValueError:
            problems.append("payload is not valid base64")
    else:
        if len(payload) > MAX_PLAIN_CHUNK:
            problems.append(
                f"plain chunk is {len(payload)} bytes (> {MAX_PLAIN_CHUNK})"
            )
        try:
            if _bad_plain_text(payload.decode("utf-8")):
                problems.append("payload has C0/DEL/C1 characters")
        except UnicodeDecodeError:
            problems.append("payload is not UTF-8")
    if ptype == "icon" and not encoded:
        problems.append("p=icon without e=1")
    if ptype in ("close", "alive", "?") and payload:
        problems.append(f"p={ptype} should have an empty payload")
    return problems


def check_transfers(sequences: list[bytes]) -> list[str]:
    """Check chunk bookkeeping across the whole run (no dangling d=0, icons decode)."""
    problems: list[str] = []
    pending: dict[str | None, dict[str, bytes]] = {}
    for sequence in sequences:
        if not sequence.startswith(b"\x1b]99;"):
            continue
        meta_b, _, payload = sequence[5:-2].partition(b";")
        fields = parse_meta(meta_b.decode("ascii"))
        ptype = fields.get("p", "title")
        if ptype not in _CONTENT_TYPES:
            continue
        key = fields.get("i")
        slot = pending.setdefault(key, {})
        slot[ptype] = slot.get(ptype, b"") + payload
        if fields.get("d", "1") == "1":
            icon = pending.pop(key).get("icon")
            if icon:
                try:
                    problems += [
                        f"icon {key}: {p}" for p in check_png(base64.b64decode(icon))
                    ]
                except ValueError:
                    problems.append(f"icon {key}: reassembled base64 is invalid")
    for key in pending:
        problems.append(f"transfer {key!r} never finished (dangling d=0)")
    return problems


def dry_run_report() -> int:
    """Validate everything sent in dry-run mode; return the process exit code."""
    print()
    print("=== dry-run validation ===")
    failures = 0
    for index, sequence in enumerate(OPTS.sent, 1):
        for problem in validate_sequence(sequence):
            failures += 1
            print(f"  sequence {index}: {problem}: {escaped(sequence)[:120]}")
    for problem in check_transfers(OPTS.sent):
        failures += 1
        print(f"  transfer: {problem}")
    for name, png in (("ICON_PNG", ICON_PNG), ("ICON_PNG_2", ICON_PNG_2)):
        for problem in check_png(png):
            failures += 1
            print(f"  {name}: {problem}")
    # Reply parser self-check, so the reading side is exercised without a tty.
    sample = b"junk\x1b]99;i=a:p=close;\x1b\\x\x9d99;i=b;2\x9c\x1b]99;i=c"
    replies, other, tail = split_replies(sample)
    ok = (
        [(r.metadata, r.payload) for r in replies]
        == [("i=a:p=close", ""), ("i=b", "2")]
        and other == b"junkx"
        and tail == b"\x1b]99;i=c"
    )
    if not ok:
        failures += 1
        print("  reply parser self-check failed")
    print(f"  {len(OPTS.sent)} sequences checked, {failures} problem(s).")
    return 1 if failures else 0


# ---------------------------------------------------------------------------
# Menu
# ---------------------------------------------------------------------------


def print_menu() -> None:
    """Print the step menu."""
    print()
    print("OSC 99 desktop notification exerciser")
    print("Run inside freminal with [notifications] enabled = true, osc_99 = true.")
    print('Expectations assume routing_osc99 = "both" on Linux/BSD (toast + desktop).')
    print("Other routings / macOS / Windows differ: see the docstring or --help.")
    print()
    for number, (title, _) in enumerate(STEPS, 1):
        print(f"  {number:>2}) {title}")
    print("   a) run all steps in order (pausing between each)")
    print("   q) quit")


def run_all() -> None:
    """Run every step, pausing between them."""
    for number, (title, _) in enumerate(STEPS, 1):
        run_step(number)
        if number != len(STEPS):
            drain_stray()
            pause("Press Enter for the next step...")


def main() -> int:
    """Entry point: parse arguments, then run the menu (or the dry run)."""
    parser = argparse.ArgumentParser(
        description="Manual OSC 99 notification exerciser (run inside freminal).",
        epilog="See the module docstring for the routing assumptions of every Expect: line.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print and validate every sequence of every step; do not touch the tty",
    )
    args = parser.parse_args()

    if args.dry_run:
        OPTS.dry_run = True
        for number in range(1, len(STEPS) + 1):
            run_step(number)
        return dry_run_report()

    if not _POSIX_TTY:
        print(
            "Note: termios/select are unavailable on this platform (likely "
            "Windows). Sequences are still sent; reply reading is skipped."
        )

    try:
        while True:
            drain_stray()
            print_menu()
            line = read_line("Select a step: ")
            if line is None:
                break
            choice = line.strip().lower()
            if choice in ("q", "quit", "exit"):
                break
            if choice == "a":
                run_all()
                continue
            if choice.isdigit() and 1 <= int(choice) <= len(STEPS):
                run_step(int(choice))
                continue
            print(f"Unrecognized choice: {choice!r}")
    except KeyboardInterrupt:
        print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
