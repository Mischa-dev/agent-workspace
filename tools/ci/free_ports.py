"""Stop this user's processes that still listen on the given TCP ports.

    python3 tools/ci/free_ports.py PORT...

checks.sh runs this before the browser smoke tests. A CI run that was
cancelled can leave its server or Vite listening on their ports, and the next
run's setup would then sign in to the old server with its own password. Only
one CI run holds a machine at a time (the lock in run.sh), so whatever this
user still has listening there is left over from an earlier run.

Linux only: it reads /proc.
"""

import os
from pathlib import Path
import signal
import sys
import time

LISTEN = "0A"


def listening_sockets(ports, uid, proc=Path("/proc")):
    """Map socket inode to port for this uid's listening sockets on `ports`."""
    found = {}
    for table in ("tcp", "tcp6"):
        try:
            lines = (proc / "net" / table).read_text().splitlines()[1:]
        except OSError:
            continue
        for line in lines:
            fields = line.split()
            if len(fields) < 10 or fields[3] != LISTEN or fields[7] != str(uid):
                continue
            port = int(fields[1].rsplit(":", 1)[1], 16)
            if port in ports and fields[9] != "0":
                found[fields[9]] = port
    return found


def holders(sockets, proc=Path("/proc")):
    """Map pid to port for the processes holding any of `sockets` open."""
    found = {}
    for entry in proc.iterdir():
        if not entry.name.isdigit():
            continue
        try:
            links = [os.readlink(fd) for fd in (entry / "fd").iterdir()]
        except OSError:
            continue  # gone, or not ours
        for link in links:
            inode = link[len("socket:[") : -1] if link.startswith("socket:[") else None
            if inode in sockets:
                found[int(entry.name)] = sockets[inode]
                break
    return found


def alive(pid):
    """True until the process has exited (a zombie counts as exited)."""
    try:
        stat = Path(f"/proc/{pid}/stat").read_text()
    except OSError:
        return False
    return stat.rpartition(")")[2].split()[0] != "Z"


def main(argv):
    ports = {int(port) for port in argv}
    pids = holders(listening_sockets(ports, os.getuid()))
    for pid, port in sorted(pids.items()):
        try:
            name = Path(f"/proc/{pid}/comm").read_text().strip()
        except OSError:
            name = "process"
        print(f"ci: stopping {name} (pid {pid}), left listening on port {port} by an earlier run")
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    deadline = time.monotonic() + 10
    while any(alive(pid) for pid in pids) and time.monotonic() < deadline:
        time.sleep(0.2)
    for pid in pids:
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
