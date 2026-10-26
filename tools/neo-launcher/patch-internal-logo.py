#!/usr/bin/env python3
"""Replaces the kernel brand logo inside Chromium's resource paks.

Chromium renders its own pages (chrome://downloads, chrome://settings, chrome://version, …)
from images compiled into `chrome_100_percent.pak` and `chrome_200_percent.pak`; the loose
`Resources/product_logo_32.png` is not what those pages ask for. The same artwork ships at
several sizes (Chromium keeps a 16px, 22px, 32px and their @2x siblings), so every copy has
to change.

A pak entry is an (offset, size) pair, so an entry can be replaced in place as long as the
new bytes are no longer than the old ones. A PNG decoder stops at IEND, so the replacement
is padded with zeroes to the original length.

The pak stores no resource names. Entries are identified by artwork instead: every square
image is shrunk to 4x4 and compared with the product logo shipped next to them. Copies of
the same artwork score above 0.99 however they are scaled; unrelated blue icons stay below
0.85.

Usage:
  patch-internal-logo.py <app bundle> <source icns|png>            replace
  patch-internal-logo.py <app bundle> --check <reference png>      report leftovers

Exit codes: 0 patched / clean, 1 failure, 3 nothing to do or leftovers found.
"""

from __future__ import annotations

import hashlib
import math
import pathlib
import struct
import subprocess
import sys
import tempfile

PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
IEND = b"IEND\xaeB`\x82"
MIN_SIDE = 16
MAX_SIDE = 256
GRID = 4
# Copies of one artwork score ~0.999 at any scale; the closest unrelated blue icon scores
# 0.83, so the gap is wide.
SIMILARITY_THRESHOLD = 0.95


def render_logo(source: pathlib.Path, size: int) -> bytes:
    """Renders `source` to a square PNG of `size` pixels.

    A straight export of the icon is larger than the pak entries allow, so the artwork goes
    through GIF first: quantising to a palette keeps this flat-colour mark small.
    """
    with tempfile.TemporaryDirectory() as directory:
        palette = pathlib.Path(directory) / "logo.gif"
        out = pathlib.Path(directory) / "logo.png"
        steps = [
            ["sips", "-s", "format", "gif", "-z", str(size), str(size), str(source), "--out", str(palette)],
            ["sips", "-s", "format", "png", str(palette), "--out", str(out)],
        ]
        for command in steps:
            subprocess.run(command, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        return out.read_bytes()


def color_grid(png: pathlib.Path) -> list[tuple[int, int, int]]:
    """The image shrunk to GRID x GRID RGB cells."""
    with tempfile.TemporaryDirectory() as directory:
        bmp = pathlib.Path(directory) / "grid.bmp"
        subprocess.run(
            ["sips", "-s", "format", "bmp", "-z", str(GRID), str(GRID), str(png), "--out", str(bmp)],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        raw = bmp.read_bytes()
        offset = struct.unpack("<I", raw[10:14])[0]
        width = struct.unpack("<i", raw[18:22])[0]
        height = struct.unpack("<i", raw[22:26])[0]
        row_size = ((24 * width + 31) // 32) * 4
        cells = []
        for y in range(abs(height)):
            for x in range(width):
                base = offset + y * row_size + x * 3
                blue, green, red = raw[base], raw[base + 1], raw[base + 2]
                cells.append((red, green, blue))
        return cells


def normalized(values: list[float]) -> list[float]:
    mean = sum(values) / len(values)
    centered = [value - mean for value in values]
    norm = math.sqrt(sum(value * value for value in centered)) or 1.0
    return [value / norm for value in centered]


def similarity(left: list[tuple[int, int, int]], right: list[tuple[int, int, int]]) -> float:
    """Correlation of two grids, averaged over the three channels."""
    if len(left) != len(right) or not left:
        return 0.0
    total = 0.0
    for channel in range(3):
        a = normalized([cell[channel] for cell in left])
        b = normalized([cell[channel] for cell in right])
        total += sum(x * y for x, y in zip(a, b))
    return total / 3


def png_entries(data: bytes) -> list[tuple[int, int, int, int]]:
    """Every PNG in a pak, as (width, height, size, offset)."""
    entries = []
    index = 0
    while True:
        index = data.find(PNG_SIGNATURE, index)
        if index < 0:
            return entries
        end = data.find(IEND, index)
        if end < 0:
            return entries
        end += len(IEND)
        width, height = struct.unpack(">II", data[index + 16 : index + 24])
        entries.append((width, height, end - index, index))
        index = end


def pak_files(app: pathlib.Path) -> list[pathlib.Path]:
    """The two colour paks, once each (Versions/Current is a symlink to the version dir)."""
    found = sorted(app.glob("Contents/Frameworks/*.framework/Versions/*/Resources/chrome_*_percent.pak"))
    unique = []
    seen = set()
    for pak in found:
        real = pak.resolve()
        if real not in seen:
            seen.add(real)
            unique.append(pak)
    return unique


def brand_entries(app: pathlib.Path, reference: list[tuple[int, int, int]]) -> list[tuple[pathlib.Path, int, int, int]]:
    """Pak entries whose artwork is the product logo: (pak, offset, size, side)."""
    matches = []
    with tempfile.TemporaryDirectory() as directory:
        scratch = pathlib.Path(directory)
        for pak in pak_files(app):
            data = pak.read_bytes()
            for width, height, size, offset in png_entries(data):
                if width != height or not MIN_SIDE <= width <= MAX_SIDE:
                    continue
                candidate = scratch / f"{pak.stem}-{offset}.png"
                candidate.write_bytes(data[offset : offset + size])
                if similarity(reference, color_grid(candidate)) >= SIMILARITY_THRESHOLD:
                    matches.append((pak, offset, size, width))
    return matches


def patch_entry(pak: pathlib.Path, offset: int, size: int, replacement: bytes) -> bool:
    if len(replacement) > size:
        print(f"  skip {pak.name}@{offset}: needs {len(replacement)} bytes, entry allows {size}")
        return False
    data = bytearray(pak.read_bytes())
    data[offset : offset + size] = replacement + b"\x00" * (size - len(replacement))
    pak.write_bytes(bytes(data))
    return True


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        return 1
    app = pathlib.Path(sys.argv[1])
    if not app.is_dir():
        print(f"missing app bundle: {app}", file=sys.stderr)
        return 1

    check_mode = sys.argv[2] == "--check"
    if check_mode:
        if len(sys.argv) != 4:
            print(__doc__, file=sys.stderr)
            return 1
        reference_png = pathlib.Path(sys.argv[3])
        if not reference_png.is_file():
            print(f"missing reference: {reference_png}", file=sys.stderr)
            return 1
        leftovers = brand_entries(app, color_grid(reference_png))
        for pak, offset, size, side in leftovers:
            print(f"  leftover {pak.name}@{offset} {side}x{side} ({size} bytes)")
        print(f"{len(leftovers)} kernel-brand entr(ies) left")
        return 0 if not leftovers else 3

    source = pathlib.Path(sys.argv[2])
    if not source.is_file():
        print(f"missing source: {source}", file=sys.stderr)
        return 1

    loose_list = sorted(app.glob("Contents/Frameworks/*.framework/Versions/*/Resources/product_logo_32.png"))
    if not loose_list:
        print("no product_logo_32.png found", file=sys.stderr)
        return 3
    original = loose_list[0].read_bytes()
    if not original.startswith(PNG_SIGNATURE):
        print(f"{loose_list[0]} is not a PNG", file=sys.stderr)
        return 1

    with tempfile.TemporaryDirectory() as directory:
        reference_png = pathlib.Path(directory) / "reference.png"
        reference_png.write_bytes(original)
        reference_grid = color_grid(reference_png)

    patched = 0
    rendered: dict[int, bytes] = {}
    for pak, offset, size, side in brand_entries(app, reference_grid):
        if side not in rendered:
            rendered[side] = render_logo(source, side)
        if patch_entry(pak, offset, size, rendered[side]):
            print(f"  {pak.name}@{offset} {side}x{side} ({size} bytes)")
            patched += 1

    for path in loose_list:
        path.write_bytes(rendered.setdefault(32, render_logo(source, 32)))

    digest = hashlib.sha256(rendered[32]).hexdigest()[:12]
    print(f"logo {digest} written to {len(loose_list)} file(s) and {patched} pak entr(ies)")

    # Prove the sweep caught everything: a kernel upgrade that adds another size would
    # show up here instead of as a stray logo nobody notices.
    leftovers = brand_entries(app, reference_grid)
    if leftovers:
        for pak, offset, size, side in leftovers:
            print(f"  leftover {pak.name}@{offset} {side}x{side} ({size} bytes)")
        return 1
    print("  no kernel-brand artwork left in the paks")
    return 0 if patched else 3


if __name__ == "__main__":
    sys.exit(main())
