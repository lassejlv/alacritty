"""Check that the ctypes signatures match the functions declared in alacritty.h."""

from pathlib import Path
import re
import sys

import alacritty_ffi

header = (Path(__file__).resolve().parents[2] / "include/alacritty.h").read_text()
header = re.sub(r"/\*.*?\*/", "", header, flags=re.S)
declared = {}
for match in re.finditer(r"\b(alac_\w+)\(([^;{]*?)\);", header):
    params = match.group(2).strip()
    declared[match.group(1)] = 0 if params in ("", "void") else params.count(",") + 1

errors = []
for name, (_, argtypes) in alacritty_ffi._SIGNATURES.items():
    if name not in declared:
        errors.append(f"{name} is not declared in alacritty.h")
    elif declared[name] != len(argtypes):
        errors.append(f"{name} takes {declared[name]} arguments, binding passes {len(argtypes)}")

constants = dict(re.findall(r"\b(ALAC_[A-Z0-9_]+) = (\d+)", header))
constants.update((name, str(1 << int(bit)))
                 for name, bit in re.findall(r"\b(ALAC_[A-Z0-9_]+) = 1 << (\d+)", header))
expected = {f"ALAC_EVENT_{name.upper()}": kind for kind, name in alacritty_ffi.EVENT_NAMES.items()}
expected.update({f"ALAC_VI_{name.upper()}": i for i, name in enumerate(alacritty_ffi.VI_MOTIONS)})
expected.update({f"ALAC_KEY_{name.upper()}": key for name, key in alacritty_ffi.KEYS.items()})
expected["ALAC_KEY_F1"] = alacritty_ffi.KEY_F1
for name in dir(alacritty_ffi):
    if name.startswith(("MODE_", "CELL_", "MOD_")):
        expected[f"ALAC_{name}"] = getattr(alacritty_ffi, name)
for name, value in expected.items():
    if int(constants.get(name, -1)) != value:
        errors.append(f"{name} is {constants.get(name)}, binding uses {value}")
if int(re.search(r"#define ALAC_ABI_VERSION (\d+)", header).group(1)) != alacritty_ffi.ABI_VERSION:
    errors.append("ABI_VERSION differs from alacritty.h")

print("\n".join(errors) or f"{len(alacritty_ffi._SIGNATURES)} signatures match")
sys.exit(1 if errors else 0)
