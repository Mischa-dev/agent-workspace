"""Write one shard's share of the browser smoke tests as a Playwright test list.

Usage: e2e_shard.py K/N < listing.json > list.txt, where listing.json is what
`playwright test --list --reporter=json` writes.

Playwright's own --shard splits the tests into N runs of neighbours, and the
slow ones (the accessibility and phone sweeps, a minute each) sit together at
the front, so one shard took five times as long as another. This deals the
tests out in turn instead (test 1 to shard 1, test 2 to shard 2, ...), which
spreads the slow neighbours across the shards. Every test lands in exactly
one shard; a title the list format can't carry fails the run rather than
dropping its test.
"""

import json
import sys


def tests(suite, titles):
    for spec in suite.get("specs", []):
        for test in spec["tests"]:
            yield test["projectName"], spec["file"], [*titles, spec["title"]]
    for child in suite.get("suites", []):
        yield from tests(child, [*titles, child["title"]])


def main():
    try:
        index, count = (int(part) for part in sys.argv[1].split("/"))
        if not 1 <= index <= count:
            raise ValueError
    except (IndexError, ValueError):
        sys.exit("usage: e2e_shard.py K/N (1 <= K <= N)")

    listing = json.load(sys.stdin)
    every = [t for file in listing["suites"] for t in tests(file, [])]
    if not every:
        sys.exit("e2e_shard: Playwright listed no tests")
    for project, file, titles in every:
        for token in (project, file, *titles):
            if "›" in token or ">" in token or token != token.strip() or not token:
                sys.exit(f"e2e_shard: can't put {token!r} in a test list ({file})")

    mine = every[index - 1 :: count]
    print(f"# shard {index}/{count}: {len(mine)} of {len(every)} tests")
    for project, file, titles in mine:
        print(" › ".join([f"[{project}]", file, *titles]))


if __name__ == "__main__":
    main()
