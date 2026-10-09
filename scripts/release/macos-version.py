"""Validate release tags and stamp bundle versions before signing."""
import os
from pathlib import Path
import plistlib
import re
import sys


def release_version(tag):
    if not re.fullmatch(r"v?(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", tag):
        raise ValueError("Release tags must be vMAJOR.MINOR.PATCH (publish as a prerelease first)")
    return tag.removeprefix("v")


def stamp(app, tag):
    path = Path(app) / "Contents/Info.plist"
    info = plistlib.loads(path.read_bytes())
    if tag:
        version = release_version(tag)
        info["CFBundleVersion"] = version
        info["CFBundleShortVersionString"] = version
    else:
        # Manual/local builds are validation snapshots, never newer than a real release.
        info["CFBundleVersion"] = "0"
    path.write_bytes(plistlib.dumps(info))


if __name__ == "__main__":
    tag = os.environ.get("RELEASE_TAG", "")
    if sys.argv[1] == "env":
        if tag:
            with open(os.environ["GITHUB_ENV"], "a") as env:
                env.write(f"ALACRITTY_RELEASE_VERSION={release_version(tag)}\n")
    else:
        stamp(sys.argv[1], tag)
