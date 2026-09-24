#!/usr/bin/env python3
"""Install one bearer credential from stdin without exposing it in arguments."""

import os
from pathlib import Path
import sys
import tempfile

slug = sys.argv[1]
if slug not in {"mara", "rook", "iona", "sable", "moss"}:
    raise SystemExit("Unknown crew member")
target = Path.home() / ".local/share/agent-workspace-bridge/tokens" / slug
target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
os.chmod(target.parent, 0o700)
token = sys.stdin.buffer.read().strip()
if not token.startswith(b"aw_") or len(token) < 30:
    raise SystemExit("Missing credential")
fd, temporary = tempfile.mkstemp(prefix=".token-", dir=target.parent)
try:
    os.fchmod(fd, 0o600)
    os.write(fd, token + b"\n")
    os.fsync(fd)
    os.close(fd)
    os.replace(temporary, target)
finally:
    if os.path.exists(temporary):
        os.unlink(temporary)
print("stored")
