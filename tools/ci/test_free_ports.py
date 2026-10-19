"""Exercise free_ports.py against real listening processes (Linux only)."""

from pathlib import Path
import socket
import subprocess
import sys
import unittest


SCRIPT = Path(__file__).with_name("free_ports.py")
LISTENER = (
    "import socket, sys, time\n"
    "s = socket.socket()\n"
    "s.bind(('127.0.0.1', 0))\n"
    "s.listen()\n"
    "print(s.getsockname()[1], flush=True)\n"
    "time.sleep(60)\n"
)


@unittest.skipUnless(Path("/proc/net/tcp").exists(), "needs Linux /proc")
class FreePortsTests(unittest.TestCase):
    def listener(self):
        process = subprocess.Popen([sys.executable, "-c", LISTENER], stdout=subprocess.PIPE, text=True)
        self.addCleanup(process.kill)
        self.addCleanup(process.stdout.close)
        return process, int(process.stdout.readline())

    def free(self, *ports):
        return subprocess.run(
            [sys.executable, str(SCRIPT), *map(str, ports)], capture_output=True, text=True, timeout=30
        )

    def test_stops_only_the_process_on_the_given_port(self):
        left_over, port = self.listener()
        bystander, other_port = self.listener()
        result = self.free(port)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"pid {left_over.pid}", result.stdout)
        self.assertEqual(left_over.wait(timeout=15), -15)
        self.assertIsNone(bystander.poll())
        with socket.create_connection(("127.0.0.1", other_port), timeout=5):
            pass

    def test_nothing_listening_is_a_no_op(self):
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        result = self.free(port)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
