#!/usr/bin/env python3
"""Build/check the bundled Noto Sans JP subset.

Tooling: uv run --with-requirements scripts/requirements-assets.txt python scripts/subset_font.py
Source setup (explicit network access): add --fetch-source
Offline verification: add --check
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import tempfile
import unicodedata
import urllib.request
from pathlib import Path

from fontTools.subset import Options, Subsetter
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parents[1]
FONT_PATH = ROOT / "assets/fonts/NotoSansJP.ttf"
SOURCE_PATH = ROOT / "target/font-source/NotoSansJP[wght].ttf"
SOURCE_URL = (
    "https://raw.githubusercontent.com/google/fonts/"
    "295d98a7a0c17c68f1341eaeea354e7960ea70d3/"
    "ofl/notosansjp/NotoSansJP%5Bwght%5D.ttf"
)
SOURCE_SHA256 = "c2f3b4d463500a2ddcd3849cded1fceeb9fd6d1c32e6cbecd568453ba50fc68f"

# These remain explicit even if app/data strings are later reorganized.
REQUIRED_SAMPLES = {
    "Japanese UI": "くに暗記、世界のかたち。",
    "country-name diacritics": "Côte d'Ivoire / São Tomé and Príncipe",
}
EXTRA_CHARACTERS = "\u00a0\u2026\u3007"
LICENSE_NAME_IDS = (0, 13, 14)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def fetch_source() -> None:
    SOURCE_PATH.parent.mkdir(parents=True, exist_ok=True)
    if SOURCE_PATH.exists() and sha256(SOURCE_PATH) == SOURCE_SHA256:
        return

    digest = hashlib.sha256()
    fd, temporary_name = tempfile.mkstemp(prefix="NotoSansJP-", dir=SOURCE_PATH.parent)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(fd, "wb") as output, urllib.request.urlopen(SOURCE_URL) as response:
            while chunk := response.read(1024 * 1024):
                digest.update(chunk)
                output.write(chunk)
        actual = digest.hexdigest()
        if actual != SOURCE_SHA256:
            raise ValueError(f"upstream font SHA-256 mismatch: {actual}")
        temporary.replace(SOURCE_PATH)
    finally:
        temporary.unlink(missing_ok=True)


def collect_text() -> set[str]:
    characters = set("".join(chr(codepoint) for codepoint in range(0x20, 0x7F)))
    characters.update(EXTRA_CHARACTERS)
    for sample in REQUIRED_SAMPLES.values():
        characters.update(sample)

    text_files = sorted((ROOT / "ui").rglob("*.slint"))
    text_files += sorted((ROOT / "src").glob("*.rs"))
    if not text_files:
        raise ValueError("no production Slint or Rust source files found")
    text_files += [
        ROOT / "docs/privacy.md",
        ROOT / "assets/flags/LICENSE",
        ROOT / "assets/fonts/OFL.txt",
    ]
    for path in text_files:
        characters.update(path.read_text(encoding="utf-8"))

    data_files = sorted((ROOT / "data/countries").glob("*.json"))
    if not data_files:
        raise ValueError("no country JSON files found")

    def add_json_text(value: object) -> None:
        if isinstance(value, str):
            characters.update(value)
        elif isinstance(value, dict):
            for key, item in value.items():
                add_json_text(key)
                add_json_text(item)
        elif isinstance(value, list):
            for item in value:
                add_json_text(item)

    for path in data_files:
        with path.open(encoding="utf-8") as source:
            add_json_text(json.load(source))

    return {character for character in characters if unicodedata.category(character) != "Cc"}


def missing_characters(font: TTFont, characters: set[str]) -> list[str]:
    cmap = font.getBestCmap() or {}
    return sorted((character for character in characters if ord(character) not in cmap), key=ord)


def describe_missing(characters: list[str]) -> str:
    return ", ".join(
        f"U+{ord(character):04X} {character!r} ({unicodedata.name(character, 'unnamed')})"
        for character in characters
    )


def axes(font: TTFont) -> tuple[tuple[str, float, float, float], ...]:
    if "fvar" not in font:
        raise ValueError("font has no fvar variable axes")
    return tuple(
        (axis.axisTag, axis.minValue, axis.defaultValue, axis.maxValue)
        for axis in font["fvar"].axes
    )


def names_for(font: TTFont, name_id: int) -> set[str]:
    return {
        record.toUnicode()
        for record in font["name"].names
        if record.nameID == name_id
    }


def validate_font(path: Path, characters: set[str], expected_axes=None, expected_names=None) -> None:
    font = TTFont(path, recalcBBoxes=False, recalcTimestamp=False)
    try:
        missing = missing_characters(font, characters)
        if missing:
            raise ValueError(f"font is missing required characters: {describe_missing(missing)}")
        actual_axes = axes(font)
        if expected_axes is not None and actual_axes != expected_axes:
            raise ValueError(f"variable axes changed: expected {expected_axes!r}, got {actual_axes!r}")
        if font["OS/2"].fsType != 0:
            raise ValueError(f"unexpected embedding restrictions: OS/2.fsType={font['OS/2'].fsType}")
        for name_id in LICENSE_NAME_IDS:
            values = names_for(font, name_id)
            if not values:
                raise ValueError(f"font is missing required name-table metadata ID {name_id}")
            if expected_names is not None and values != expected_names[name_id]:
                raise ValueError(f"font name-table metadata ID {name_id} changed")
    finally:
        font.close()


def subset(source_path: Path, characters: set[str]) -> None:
    if not source_path.is_file():
        raise FileNotFoundError(
            f"missing pinned source font at {source_path}; fetch it explicitly with --fetch-source"
        )
    actual_hash = sha256(source_path)
    if actual_hash != SOURCE_SHA256:
        raise ValueError(f"source font SHA-256 mismatch: expected {SOURCE_SHA256}, got {actual_hash}")

    font = TTFont(source_path, recalcBBoxes=False, recalcTimestamp=False)
    try:
        required = missing_characters(font, characters)
        if required:
            raise ValueError(f"source font is missing required characters: {describe_missing(required)}")
        expected_axes = axes(font)
        expected_names = {name_id: names_for(font, name_id) for name_id in LICENSE_NAME_IDS}
        if any(not values for values in expected_names.values()):
            raise ValueError("source font lacks copyright or OFL metadata")

        options = Options()
        options.layout_features = ["*"]
        options.name_IDs = ["*"]
        options.name_languages = ["*"]
        options.name_legacy = True
        options.glyph_names = True
        options.legacy_cmap = True
        options.symbol_cmap = True
        options.notdef_glyph = True
        options.notdef_outline = True
        options.recommended_glyphs = True
        subsetter = Subsetter(options=options)
        subsetter.populate(unicodes={ord(character) for character in characters})
        subsetter.subset(font)

        FONT_PATH.parent.mkdir(parents=True, exist_ok=True)
        fd, temporary_name = tempfile.mkstemp(prefix="NotoSansJP-", suffix=".ttf", dir=FONT_PATH.parent)
        os.close(fd)
        temporary = Path(temporary_name)
        try:
            font.save(temporary)
            validate_font(temporary, characters, expected_axes, expected_names)
            temporary.replace(FONT_PATH)
        finally:
            temporary.unlink(missing_ok=True)
    finally:
        font.close()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="check the bundled font without network access")
    mode.add_argument("--fetch-source", action="store_true", help="fetch the pinned upstream source explicitly")
    parser.add_argument("--source", type=Path, help="use a local copy of the pinned upstream source")
    args = parser.parse_args()
    if args.fetch_source and args.source:
        parser.error("--source cannot be combined with --fetch-source")

    characters = collect_text()
    if args.check:
        validate_font(FONT_PATH, characters)
        print(f"OK: {len(characters)} required characters covered by {FONT_PATH} ({FONT_PATH.stat().st_size} bytes)")
        return

    if args.fetch_source:
        fetch_source()
    subset(args.source or SOURCE_PATH, characters)
    print(f"Wrote {FONT_PATH} ({FONT_PATH.stat().st_size} bytes; {len(characters)} required characters)")


if __name__ == "__main__":
    main()
