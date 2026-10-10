#!/usr/bin/env python3
"""Convert Ghostty theme files into Alacritty built-in themes.

Usage: scripts/themes/ghostty-to-toml.py OUT_DIR GHOSTTY_THEME...

Each output is named after the Ghostty theme in kebab case, so "TokyoNight Storm" becomes
"tokyo-night-storm.toml". Only color settings are converted.
"""
from pathlib import Path
import re
import sys

ANSI = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"]
SOURCE = "https://github.com/mbadolato/iTerm2-Color-Schemes/blob/master/ghostty/"
SCALARS = {
    "background": ("primary", "background"),
    "foreground": ("primary", "foreground"),
    "cursor-color": ("cursor", "background"),
    "cursor-text": ("cursor", "foreground"),
    "selection-background": ("selection", "background"),
    "selection-foreground": ("selection", "foreground"),
}


def color(value):
    value = value.strip().removeprefix("#")
    if len(value) != 6 or any(c not in "0123456789abcdefABCDEF" for c in value):
        raise ValueError(f"invalid color {value!r}")
    return f"#{value.lower()}"


def kebab(name):
    # Keep brand names whole and drop the iTerm2 prefix of its Solarized variants.
    name = name.replace("GitHub", "Github").removeprefix("iTerm2 ")
    name = re.sub(r"(?<=[a-z0-9])(?=[A-Z])", "-", name)
    return re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")


def convert(path):
    tables = {}
    indexed = {}
    for line in path.read_text().splitlines():
        key, sep, value = line.partition("=")
        key = key.strip()
        if not sep or key.startswith("#"):
            continue
        if key == "palette":
            index, _, value = value.partition("=")
            index = int(index)
            if index < 16:
                table = "normal" if index < 8 else "bright"
                tables.setdefault(table, {})[ANSI[index % 8]] = color(value)
            else:
                indexed[index] = color(value)
        elif key in SCALARS:
            table, field = SCALARS[key]
            tables.setdefault(table, {})[field] = color(value)

    lines = [
        f'# Converted from Ghostty\'s "{path.name}" theme.',
        f"# Source: {SOURCE}{path.name.replace(' ', '%20')}",
    ]
    for table in ["primary", "cursor", "selection", "normal", "bright"]:
        if table in tables:
            lines += ["", f"[colors.{table}]"]
            lines += [f'{field} = "{value}"' for field, value in tables[table].items()]
    for index, value in sorted(indexed.items()):
        lines += ["", "[[colors.indexed_colors]]", f"index = {index}", f'color = "{value}"']
    return "\n".join(lines) + "\n"


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__.strip())
    out_dir = Path(sys.argv[1])
    for source in map(Path, sys.argv[2:]):
        target = out_dir / f"{kebab(source.name)}.toml"
        target.write_text(convert(source))
        print(target)


if __name__ == "__main__":
    main()
