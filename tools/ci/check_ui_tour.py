"""Check what the opt-in CI UI tour made (checks.sh run_ui_tour), and summarise it.

    python3 tools/ci/check_ui_tour.py <e2e output dir> <started, Unix seconds>

The tour ran with UI_TOUR_ONLY=channel: its whole seed, then the channel
screen at 1440 and 390 px in light and dark. This fails unless the seed made a
run and warned about nothing, and the four channel screenshots and the contact
sheet are this run's (written since <started>) and are what they say. It then
writes tour/summary.txt: the run's ID and each file's size and SHA-256. Nothing
from the signed-in browser state, and none of the seeded text, goes in it.
"""

import hashlib
import json
from pathlib import Path
import sys

SCREENS = [
    f"02-channel-{size}-{scheme}.png"
    for size in ("desktop", "phone")
    for scheme in ("light", "dark")
]
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


def check(output: Path, started: int) -> tuple[list[str], list[str]]:
    """The problems found, and the summary's lines."""
    problems: list[str] = []
    lines: list[str] = []

    state_path = output / "state" / "tour.json"
    try:
        state = json.loads(state_path.read_text())
    except (OSError, ValueError) as error:
        return [f"no tour seed state at {state_path} ({error})"], lines
    run_id = state.get("runId")
    if not isinstance(run_id, str) or not run_id:
        problems.append("the tour seed made no run (runId is null)")
    warnings = state.get("warnings")
    if not isinstance(warnings, list):
        problems.append("the tour seed state doesn't say what it warned about")
    elif warnings:
        problems.append(f"the tour seed warned {len(warnings)} time(s): " + "; ".join(map(str, warnings)))
    lines.append(f"runId: {run_id if isinstance(run_id, str) else 'none'}")
    lines.append(f"seed warnings: {len(warnings) if isinstance(warnings, list) else 'unknown'}")

    tour = output / "tour"
    for name in [*SCREENS, "index.html"]:
        path = tour / name
        try:
            data = path.read_bytes()
            written = int(path.stat().st_mtime)
        except OSError:
            problems.append(f"missing {name}")
            continue
        if written < started:
            problems.append(f"{name} is from before this run")
        if name.endswith(".png") and not data.startswith(PNG_SIGNATURE):
            problems.append(f"{name} isn't a PNG")
        lines.append(f"{name}: {len(data)} bytes, sha256 {hashlib.sha256(data).hexdigest()}")
    index = tour / "index.html"
    if index.is_file():
        text = index.read_text(errors="replace")
        for name in SCREENS:
            if name not in text:
                problems.append(f"index.html doesn't show {name}")
    return problems, lines


def main(argv: list[str]) -> int:
    if len(argv) != 3 or not argv[2].isdigit():
        print("usage: check_ui_tour.py <e2e output dir> <started, Unix seconds>", file=sys.stderr)
        return 2
    output, started = Path(argv[1]), int(argv[2])
    problems, lines = check(output, started)
    summary = "\n".join(["UI tour (channel)", *lines, *(f"problem: {p}" for p in problems)]) + "\n"
    if (output / "tour").is_dir():
        (output / "tour" / "summary.txt").write_text(summary)
    print(summary, end="")
    if problems:
        print(f"✗ UI tour: {len(problems)} problem(s)", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
