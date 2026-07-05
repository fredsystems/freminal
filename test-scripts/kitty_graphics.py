#!/usr/bin/python3

"""Manual exerciser for the kitty graphics protocol (Task 100).

Run this INSIDE a freminal window. This script is NOT a unit test and is NOT
wired into CI: it emits real kitty graphics APC escape sequences to your
terminal and, for most steps, reads the response bytes freminal writes back
to the PTY (`i=<id>[,I=<n>][,p=<p>] ; OK-or-ERROR`). A human has to watch the
terminal to confirm images actually appear/animate/move/vanish as expected;
the response bytes alone only prove the protocol-level bookkeeping succeeded.

Reference: `Documents/KITTY_PROTOCOL_REFERENCE.md`, "Graphics protocol
completion" section -- the envelope, the action-dependent control key table,
animation semantics, unicode placeholders, relative placements, delete
targets, transmission media, response format, compression, and storage
quotas are all documented there.

Usage:
    python3 test-scripts/kitty_graphics.py

The script presents a numbered menu (or `a` to run all steps in order,
pausing between each). Watch the terminal for the described visual result
and compare the printed wire bytes / responses against the reference doc.

Response reading uses `termios`/`tty`/`select` and is POSIX-only. On
platforms without those modules (e.g. Windows) the send-only steps still
work; steps that read a response will note that response reading is
unavailable there.
"""

from __future__ import annotations

import base64
import sys
import time
import zlib

try:
    import select
    import termios
    import tty

    _POSIX_TTY = True
except ImportError:  # pragma: no cover - Windows has no termios/tty
    _POSIX_TTY = False

ESC = "\x1b"
ST = "\x1b\\"  # String Terminator: ESC backslash
APC_START = "\x1b_G"  # APC (ESC _) followed by the kitty graphics marker 'G'

# Row/column diacritics for unicode placeholders (Task 100.3). This is only
# the first two entries of the full `rowcolumn-diacritics.txt` table
# (khaledhosny/rowcolumn-diacritics, also used by kitty/iTerm2); that's all
# that's needed to address a 2x2 grid (rows/cols 0 and 1) in step 9.
ROWCOL_DIACRITICS = ["\u0305", "\u030d"]  # index 0 -> row/col 0, index 1 -> row/col 1
PLACEHOLDER_CODEPOINT = "\U0010eeee"


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


def emit_apc(control: str, payload: bytes = b"") -> None:
    """Build and write one kitty graphics APC escape sequence.

    `control` is the already-comma-joined `key=value` control string (never
    base64 itself). `payload` is raw bytes -- pixel data, a PNG, a file
    path, or a shared-memory object name -- which this function base64-
    encodes per the wire format. An empty payload omits the `;` separator
    entirely (freminal's parser tolerates both forms).
    """
    if payload:
        b64_payload = base64.b64encode(payload).decode("ascii")
        sequence = f"{APC_START}{control};{b64_payload}{ST}".encode("ascii")
    else:
        sequence = f"{APC_START}{control}{ST}".encode("ascii")
    print(f"    sending: {escaped(sequence)}")
    sys.stdout.buffer.write(sequence)
    sys.stdout.buffer.flush()


def read_response(timeout: float = 3.0) -> bytes:
    """Read whatever bytes are available on stdin within `timeout` seconds.

    Puts the tty into raw mode so escape sequences freminal writes back
    (the `i=<id>[,I=<n>][,p=<p>] ; OK-or-ERROR` responses) arrive unmangled.
    Always restores the original terminal mode. POSIX-only; returns b"" with
    a printed note on platforms without termios/tty/select.
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
            "    no response received (was q=1/q=2 set, suppressing it? is "
            "this actually a freminal window?)"
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
# Pixel data generators (stdlib only, no image libraries)
# ---------------------------------------------------------------------------


def rgba_solid(width: int, height: int, color: tuple[int, int, int, int]) -> bytes:
    """Raw RGBA pixel data (f=32) for a `width` x `height` solid-color image."""
    return bytes(color) * (width * height)


def rgba_quadrants(size: int) -> bytes:
    """Raw RGBA pixel data for a `size` x `size` four-quadrant test image.

    Top-left = red, top-right = green, bottom-left = blue, bottom-right =
    yellow. `size` must be even. Used so that a source-rect crop (step 5,
    step 14) produces a visibly different result than the full image.
    """
    half = size // 2
    red = (255, 0, 0, 255)
    green = (0, 255, 0, 255)
    blue = (0, 0, 255, 255)
    yellow = (255, 255, 0, 255)
    out = bytearray()
    for y in range(size):
        for x in range(size):
            if x < half and y < half:
                out.extend(red)
            elif x >= half and y < half:
                out.extend(green)
            elif x < half and y >= half:
                out.extend(blue)
            else:
                out.extend(yellow)
    return bytes(out)


def sgr_fg_truecolor(image_id: int) -> str:
    """SGR truecolor foreground escape carrying `image_id` in R/G/B."""
    r = (image_id >> 16) & 0xFF
    g = (image_id >> 8) & 0xFF
    b = image_id & 0xFF
    return f"{ESC}[38;2;{r};{g};{b}m"


def placeholder_cell(row: int, col: int) -> str:
    """One unicode-placeholder cell: codepoint plus row/col diacritics."""
    return PLACEHOLDER_CODEPOINT + ROWCOL_DIACRITICS[row] + ROWCOL_DIACRITICS[col]


# ---------------------------------------------------------------------------
# Individual test steps
# ---------------------------------------------------------------------------


def step_01_query() -> None:
    step_header(
        1,
        "Query support (a=q)",
        "A query never displays or stores anything -- it just asks 'could "
        "you handle this?'. Expect a bare OK response for i=1, f=32.",
    )
    emit_apc("a=q,f=32,s=1,v=1,i=1", rgba_solid(1, 1, (255, 255, 255, 255)))
    print_response(read_response())


def step_02_transmit_and_display() -> None:
    step_header(
        2,
        "Transmit + display a small RGBA image (a=T)",
        "A 4x4 four-quadrant image (red/green/blue/yellow corners) appears "
        "at the cursor as image id 1. Expect an OK response for i=1.",
    )
    emit_apc("a=T,f=32,s=4,v=4,i=1", rgba_quadrants(4))
    print_response(read_response())


def step_03_transmit_then_put() -> None:
    step_header(
        3,
        "Transmit-only (a=t) then Put (a=p)",
        "Transmit a 4x4 solid green image as id 2 with a=t -- nothing "
        "should appear yet. Then a=p,i=2 displays it at the cursor. Expect "
        "an OK for both the transmit and the put.",
    )
    emit_apc("a=t,f=32,s=4,v=4,i=2", rgba_solid(4, 4, (0, 255, 0, 255)))
    print_response(read_response())
    print("    Nothing should be visible yet (transmit-only). Putting now...")
    emit_apc("a=p,i=2")
    print_response(read_response())


def step_04_put_with_size_override() -> None:
    step_header(
        4,
        "Put with display size override (c=/r=)",
        "Puts image 2 (the solid green 4x4 from step 3) again, stretched "
        "to 8 columns x 4 rows via c=8,r=4. Expect the same green image, "
        "wider than before, plus an OK response.",
    )
    emit_apc("a=p,i=2,c=8,r=4")
    print_response(read_response())


def step_05_put_source_rect_crop() -> None:
    step_header(
        5,
        "Put with source-rect crop straddling all four quadrants (x/y/w/h)",
        "Puts image 1 (the four-quadrant image from step 2) cropped to the "
        "2x2 pixel region starting at (1,1). Since the quadrant boundary is "
        "at pixel 2, this crop takes one pixel from each quadrant: expect a "
        "small 2x2 swatch containing one red, one green, one blue, and one "
        "yellow pixel -- NOT a clean single-color square.",
    )
    emit_apc("a=p,i=1,x=1,y=1,w=2,h=2")
    print_response(read_response())


def step_06_image_number_reference() -> None:
    step_header(
        6,
        "Image number reference (I=)",
        "Transmits a 2x2 solid purple image with I=13 and no i= -- "
        "freminal assigns a fresh image id and records it as the newest "
        "image numbered 13. Expect the response to echo i=<assigned-id>,"
        "I=13. Then a=p,I=13 puts that same image by number: expect the "
        "purple square to appear, echoing the same i=,I=13 pair.",
    )
    emit_apc("a=T,f=32,s=2,v=2,I=13", rgba_solid(2, 2, (128, 0, 128, 255)))
    print_response(read_response())
    print("    Putting by number (I=13) now...")
    emit_apc("a=p,I=13")
    print_response(read_response())


def step_07_animation() -> None:
    step_header(
        7,
        "Animation: frames (a=f), run/stop (a=a)",
        "Transmits a 4x4 red image as id 3 (root/frame 1), adds a green "
        "frame 2 and a blue frame 3 (each with a 500ms gap), then runs the "
        "animation (a=a,s=3,v=1 = run, loop forever). Expect the square at "
        "id 3 to cycle red -> green -> blue -> red... Press Enter to stop "
        "(a=a,s=1) once you've watched it cycle a few times.",
    )
    emit_apc("a=T,f=32,s=4,v=4,i=3", rgba_solid(4, 4, (255, 0, 0, 255)))
    print_response(read_response())
    emit_apc("a=f,i=3,s=4,v=4,z=500", rgba_solid(4, 4, (0, 255, 0, 255)))
    print_response(read_response())
    emit_apc("a=f,i=3,s=4,v=4,z=500", rgba_solid(4, 4, (0, 0, 255, 255)))
    print_response(read_response())
    print("    Running the animation (infinite loop)...")
    emit_apc("a=a,i=3,s=3,v=1")
    print_response(read_response())
    pause("Watch it cycle red/green/blue, then press Enter to stop it...")
    emit_apc("a=a,i=3,s=1")
    print_response(read_response())


def step_08_animation_compose() -> None:
    step_header(
        8,
        "Animation compose (a=c)",
        "Composes frame 1 (the red root frame of image 3) onto frame 2 "
        "(currently green), full-size, alpha-blended. Since red is fully "
        "opaque this overwrites frame 2 with red. Expect an OK response; "
        "re-running the animation (as in step 7) would now show "
        "red -> red -> blue instead of red -> green -> blue.",
    )
    emit_apc("a=c,i=3,r=1,c=2")
    print_response(read_response())


def step_09_unicode_placeholder() -> None:
    step_header(
        9,
        "Unicode placeholder (U=1) -- subtlest step, best-effort",
        "Transmits a 2x2 four-quadrant image quietly (q=2, id 4) and "
        "creates a 2x2-cell virtual placement (U=1,c=2,r=2). Then prints "
        "four placeholder cells (U+10EEEE + row/col diacritics) with an "
        "SGR truecolor foreground that carries the image id in its R/G/B "
        "channels. A compliant renderer substitutes the four quadrant "
        "pixels for those cells instead of drawing the (near-black, "
        "id-encoded) foreground color as text. This only demonstrates the "
        "first two diacritic table entries (row/col 0 and 1), which is all "
        "a 2x2 grid needs -- the full rowcolumn-diacritics.txt table has "
        "hundreds of entries for larger grids.",
    )
    image_id = 4
    emit_apc(f"a=t,f=32,s=2,v=2,i={image_id},q=2", rgba_quadrants(2))
    emit_apc(f"a=p,i={image_id},U=1,c=2,r=2")
    print_response(read_response())
    sequence = sgr_fg_truecolor(image_id)
    for row in range(2):
        for col in range(2):
            sequence += placeholder_cell(row, col)
        sequence += "\n"
    sequence += f"{ESC}[0m"
    print(f"    sending placeholder cells: {escaped(sequence.encode('utf-8'))}")
    sys.stdout.write(sequence)
    sys.stdout.flush()


def step_10_relative_placement() -> None:
    step_header(
        10,
        "Relative placement (real parent)",
        "Transmits + displays a 4x4 cyan image as id 5. Transmits a 4x4 "
        "magenta image as id 6 (not yet displayed), then places it "
        "relative to id 5's placement (P=5), offset 2 columns right and "
        "1 row down (H=2,V=1). Expect the magenta square to appear offset "
        "from the cyan one by roughly that amount, and the cursor to NOT "
        "have moved after the relative placement.",
    )
    emit_apc("a=T,f=32,s=4,v=4,i=5", rgba_solid(4, 4, (0, 255, 255, 255)))
    print_response(read_response())
    emit_apc("a=t,f=32,s=4,v=4,i=6", rgba_solid(4, 4, (255, 0, 255, 255)))
    print_response(read_response())
    print("    Placing image 6 relative to image 5's placement...")
    emit_apc("a=p,i=6,P=5,H=2,V=1")
    print_response(read_response())


def step_11_relative_placement_errors() -> None:
    step_header(
        11,
        "Relative placement error cases (ENOPARENT / ECYCLE / ETOODEEP)",
        "Three error paths: (1) a placement whose P= names a nonexistent "
        "image -- expect ENOPARENT. (2) a new placement of image 5 with "
        "P=6, which would close the loop from step 10 (5 -> 6 -> 5) -- "
        "expect ECYCLE. (3) a fresh chain of 9 relative placements (each "
        "child of the previous), exceeding the required minimum depth of "
        "8 -- expect ETOODEEP on the last link. No images should appear "
        "for any of these three.",
    )
    emit_apc("a=t,f=32,s=1,v=1,i=7", rgba_solid(1, 1, (255, 255, 255, 255)))
    print_response(read_response())
    print("    (1) ENOPARENT: parent image 999 does not exist...")
    emit_apc("a=p,i=7,P=999")
    print_response(read_response())

    print("    (2) ECYCLE: placing a new placement of image 5 under image 6...")
    emit_apc("a=p,i=5,p=2,P=6")
    print_response(read_response())

    print("    (3) ETOODEEP: building a 9-deep relative placement chain...")
    base_id = 30
    for depth in range(9):
        image_id = base_id + depth
        emit_apc(
            f"a=t,f=32,s=1,v=1,i={image_id}",
            rgba_solid(1, 1, (depth * 25 % 256, 0, 0, 255)),
        )
        print_response(read_response())
        if depth == 0:
            emit_apc(f"a=p,i={image_id}")
        else:
            emit_apc(f"a=p,i={image_id},P={image_id - 1}")
        print_response(read_response())


def step_12_zlib_compression() -> None:
    step_header(
        12,
        "Zlib compression (o=z)",
        "Transmits a 4x4 solid purple image as id 8, RFC 1950 zlib-"
        "compressed (o=z) before base64. Expect freminal to inflate it "
        "and the purple square to appear at the cursor, plus an OK.",
    )
    raw = rgba_solid(4, 4, (128, 0, 128, 255))
    compressed = zlib.compress(raw)
    emit_apc("a=T,f=32,s=4,v=4,i=8,o=z", compressed)
    print_response(read_response())


def step_13_shared_memory() -> None:
    step_header(
        13,
        "Shared memory (t=s)",
        "Creates a POSIX shared-memory object via "
        "multiprocessing.shared_memory (stdlib), writes a 4x4 orange "
        "image into it, then transmits with t=s, the payload being the "
        "object's name. Expect the orange square to appear and an OK "
        "response; freminal unlinks the shm object after a successful "
        "read, so our own unlink may report 'already gone' -- that's "
        "expected and handled below.",
    )
    try:
        import multiprocessing.shared_memory as shm_module
    except ImportError:  # pragma: no cover - stdlib since Python 3.8
        print("    multiprocessing.shared_memory is unavailable; skipping.")
        return

    raw = rgba_solid(4, 4, (255, 165, 0, 255))
    shm = shm_module.SharedMemory(create=True, size=len(raw))
    # Python's public `.name` strips the leading slash it prepends
    # internally on POSIX before calling `shm_open`; freminal's
    # `shm_open`/`shm_name_is_safe` expect that leading slash on the wire,
    # so it is restored here.
    shm_name = f"/{shm.name}"
    try:
        shm.buf[: len(raw)] = raw
        print(f"    created POSIX shm object {shm_name!r} ({len(raw)} bytes)")
        emit_apc(f"a=T,f=32,s=4,v=4,i=9,t=s,S={len(raw)}", shm_name.encode("ascii"))
        print_response(read_response())
    finally:
        shm.close()
        try:
            shm.unlink()
            print(f"    unlinked shm object {shm_name!r}")
        except FileNotFoundError:
            print(
                f"    shm object {shm_name!r} was already unlinked "
                "(freminal unlinks it after a successful read -- expected)"
            )


def step_14_clean_quadrant_crop() -> None:
    step_header(
        14,
        "Source-rect crop: clean single-quadrant extraction",
        "Transmits a larger 8x8 four-quadrant image as id 15, then puts "
        "it cropped to exactly its top-left quadrant (x=0,y=0,w=4,h=4). "
        "Unlike step 5's straddling crop, this should show a CLEAN solid "
        "red square -- contrast the two crop behaviors.",
    )
    emit_apc("a=t,f=32,s=8,v=8,i=15", rgba_quadrants(8))
    print_response(read_response())
    emit_apc("a=p,i=15,x=0,y=0,w=4,h=4")
    print_response(read_response())


def step_15_delete() -> None:
    step_header(
        15,
        "Delete (d=i / d=I / d=a)",
        "First d=i,i=1 removes image 1's placements but keeps its pixel "
        "data (the four-quadrant square from step 2 should vanish). Then "
        "d=I,i=1 would free the data too (image already placement-less, "
        "so this is a no-op visually but exercises the data-free path). "
        "Finally d=a removes every remaining visible placement -- expect "
        "everything on screen from earlier steps to disappear. None of "
        "these produce a response (delete does not echo i=/OK).",
    )
    emit_apc("d=i,i=1")
    print("    Image 1's placement should be gone; data is retained.")
    pause("Press Enter to continue...")
    emit_apc("d=I,i=1")
    print("    Image 1's data is now freed too (no visible change expected).")
    pause("Press Enter to delete everything else...")
    emit_apc("d=a")
    print("    All remaining visible placements should now be gone.")


def step_16_quota_stress() -> None:
    step_header(
        16,
        "Storage quota (light exercise, not a full eviction demo)",
        "Transmits five moderately-sized (32x32) images in quick "
        "succession (ids 90-94). This is a DoS-guard memory-pressure path "
        "(320MB base budget, 5x for animation frames per "
        "KITTY_PROTOCOL_REFERENCE.md) -- reliably forcing LRU eviction "
        "from a script isn't practical, so this step just exercises the "
        "insert path and confirms all five transmit and display "
        "successfully (five colored 32x32 squares, one per color).",
    )
    colors = [
        (255, 0, 0, 255),
        (0, 255, 0, 255),
        (0, 0, 255, 255),
        (255, 255, 0, 255),
        (0, 255, 255, 255),
    ]
    for offset, color in enumerate(colors):
        image_id = 90 + offset
        emit_apc(f"a=T,f=32,s=32,v=32,i={image_id}", rgba_solid(32, 32, color))
        print_response(read_response())


STEPS = [
    (1, "Query support (a=q)", step_01_query),
    (2, "Transmit + display (a=T)", step_02_transmit_and_display),
    (3, "Transmit-only (a=t) then Put (a=p)", step_03_transmit_then_put),
    (4, "Put with display size override (c=/r=)", step_04_put_with_size_override),
    (5, "Put source-rect crop, straddling quadrants", step_05_put_source_rect_crop),
    (6, "Image number reference (I=)", step_06_image_number_reference),
    (7, "Animation frames + run/stop (a=f/a=a)", step_07_animation),
    (8, "Animation compose (a=c)", step_08_animation_compose),
    (9, "Unicode placeholder (U=1)", step_09_unicode_placeholder),
    (10, "Relative placement (real parent)", step_10_relative_placement),
    (11, "Relative placement errors (ENOPARENT/ECYCLE/ETOODEEP)", step_11_relative_placement_errors),
    (12, "Zlib compression (o=z)", step_12_zlib_compression),
    (13, "Shared memory (t=s)", step_13_shared_memory),
    (14, "Clean single-quadrant crop", step_14_clean_quadrant_crop),
    (15, "Delete (d=i/d=I/d=a)", step_15_delete),
    (16, "Storage quota (light exercise)", step_16_quota_stress),
]


def print_menu() -> None:
    print()
    print("Kitty graphics protocol exerciser")
    print("Run inside a freminal window.")
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
            "(likely Windows). Send-only steps still work; steps that read "
            "a response will skip response reading."
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
