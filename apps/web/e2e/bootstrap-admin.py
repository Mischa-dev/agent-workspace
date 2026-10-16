"""Run `agent-workspace-server bootstrap-admin` on a pseudo-terminal.

bootstrap-admin only reads its password from an interactive terminal. The e2e
global setup gives this script the password on stdin (one line), and the
script answers both prompts on a pty, so the password never appears in an
argument or environment variable. Usage:

    python3 bootstrap-admin.py <server-binary> bootstrap-admin --login-name ...

Prints what the command printed (its JSON result), without the prompts.
"""

import os
import pty
import sys
import time

TIMEOUT_SECONDS = 120


def main() -> int:
    password = sys.stdin.readline().rstrip("\n").encode()
    command = sys.argv[1:]
    if not password or not command:
        print("usage: bootstrap-admin.py <binary> bootstrap-admin ... (password on stdin)", file=sys.stderr)
        return 2

    pid, fd = pty.fork()
    if pid == 0:
        os.execv(command[0], command)

    output = b""
    answered = 0
    deadline = time.monotonic() + TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        try:
            chunk = os.read(fd, 4096)
        except OSError:
            break  # the child closed the terminal
        if not chunk:
            break
        output += chunk
        # "Password: " and then "Confirm password: ".
        while answered < 2 and output.lower().count(b"password: ") > answered:
            os.write(fd, password + b"\n")
            answered += 1
    else:
        os.kill(pid, 9)
        print("bootstrap-admin timed out", file=sys.stderr)

    _, status = os.waitpid(pid, 0)
    text = output.decode(errors="replace").replace("\r\n", "\n")
    lines = [line for line in text.split("\n") if "password: " not in line.lower()]
    sys.stdout.write("\n".join(lines))
    return os.waitstatus_to_exitcode(status)


if __name__ == "__main__":
    sys.exit(main())
