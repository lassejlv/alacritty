"""Check the CI signing key against the public key embedded in the built app."""
import base64
import os
from pathlib import Path
import plistlib
import subprocess
import sys

info = plistlib.loads((Path(sys.argv[1]) / "Contents/Info.plist").read_bytes())
seed = base64.b64decode(os.environ["SPARKLE_PRIVATE_KEY"].strip(), validate=True)
if len(seed) != 32:
    raise SystemExit("Expected a 32-byte Sparkle Ed25519 seed; do not rotate the key implicitly")
# RFC 8410 PKCS#8 wrapper for the Ed25519 seed. Key material stays in stdin/memory.
result = subprocess.run(
    ["openssl", "pkey", "-inform", "DER", "-pubout", "-outform", "DER"],
    input=bytes.fromhex("302e020100300506032b657004220420") + seed,
    capture_output=True, check=True,
)
expected = bytes.fromhex("302a300506032b6570032100") + base64.b64decode(info["SUPublicEDKey"])
if result.stdout != expected:
    raise SystemExit("Sparkle signing key does not match the app's public key")
print("Sparkle signing key matches the embedded public key.")
