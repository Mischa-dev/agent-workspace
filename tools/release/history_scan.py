#!/usr/bin/env python3
"""Scans every file version and commit message in a branch's history for
secrets and for personal or infrastructure details (H1.39), so the owner can
choose to publish the history as it is or rewrite it.

    python3 tools/release/history_scan.py [--ref origin/main] [--extra FILE] > report.md

`--extra` names a file kept outside the repository with one regular
expression per line (`name<TAB>pattern`) for what is specific to this
project's people and machines: names, domains, host names. Those never go
in a tracked file. The report gives each finding's kind, where it is and in
how many file versions, and a redacted sample, never the value itself;
keep it private all the same.
"""

import argparse
import collections
import re
import subprocess
import sys

PATTERNS = {
    "private key": r"-----BEGIN (?:RSA |EC |OPENSSH |DSA |PGP )?PRIVATE KEY",
    "AWS access key": r"\bAKIA[0-9A-Z]{16}\b",
    "GitHub token": r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{40,})\b",
    "Anthropic key": r"\bsk-ant-[A-Za-z0-9_-]{20,}",
    "OpenAI-style key": r"\bsk-(?:proj-)?[A-Za-z0-9]{32,}\b",
    "Slack token": r"\bxox[abprs]-[A-Za-z0-9-]{10,}",
    "Google API key": r"\bAIza[0-9A-Za-z_-]{35}\b",
    "bearer or password literal": r"(?i)(?:password|passwd|secret|token|api[_-]?key)\s*[:=]\s*[\"'][^\"'\s]{12,}[\"']",
    "tailnet address": r"(?<![\d.])100\.(?:6[4-9]|[7-9]\d|1[01]\d|12[0-7])\.\d{1,3}\.\d{1,3}(?![\d])",
    "tailnet host name": r"\b[a-z0-9-]+\.[a-z0-9-]+\.ts\.net\b",
    "home directory": r"(?:/home|/Users)/[a-z_][a-z0-9_.-]*/",
    "email address": r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b",
}
# Addresses and paths that are placeholders or public by design.
HARMLESS = re.compile(
    r"(?i)(?:@(?:example\.(?:com|org|net|test)|[a-z0-9.-]*\.test|[a-z0-9.-]*\.invalid|"
    r"users\.noreply\.github\.com|anthropic\.com|localhost)|noreply@|/home/(?:user|runner|node|ubuntu|agent-workspace)/|"
    r"/Users/(?:you|me|name|example)/|<[a-z]+>)"
)


def git(*args, binary=False):
    output = subprocess.run(["git", *args], check=True, capture_output=True).stdout
    return output if binary else output.decode("utf-8", "replace")


def redact(value):
    return value[:4] + "…" if len(value) > 6 else "…"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--ref", default="origin/main")
    parser.add_argument("--extra")
    options = parser.parse_args()
    patterns = dict(PATTERNS)
    if options.extra:
        for line in open(options.extra, encoding="utf-8"):
            if line.strip() and not line.startswith("#"):
                name, pattern = line.rstrip("\n").split("\t", 1)
                patterns[f"owner: {name}"] = pattern
    compiled = {name: re.compile(pattern) for name, pattern in patterns.items()}

    # name -> path -> [versions seen, sample]
    found = collections.defaultdict(lambda: collections.defaultdict(lambda: [0, ""]))
    paths = collections.defaultdict(set)
    for line in git("rev-list", "--objects", options.ref).splitlines():
        sha, _, path = line.partition(" ")
        if path:
            paths[sha].add(path)
    blobs = [sha for sha in paths]
    checked = 0
    process = subprocess.Popen(["git", "cat-file", "--batch"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    for sha in blobs:
        process.stdin.write((sha + "\n").encode())
        process.stdin.flush()
        header = process.stdout.readline().decode().split()
        size = int(header[2]) if len(header) == 3 else 0
        body = process.stdout.read(size)
        process.stdout.read(1)
        if len(header) != 3 or header[1] != "blob" or b"\0" in body[:8000] or size > 4_000_000:
            continue
        checked += 1
        text = body.decode("utf-8", "replace")
        for name, pattern in compiled.items():
            for match in pattern.finditer(text):
                value = match.group(0)
                if HARMLESS.search(value):
                    continue
                for path in paths[sha]:
                    entry = found[name][path]
                    entry[0] += 1
                    entry[1] = entry[1] or redact(value)
                break
    process.stdin.close()
    process.wait()

    messages = git("log", "--format=%H%x00%B%x00", options.ref).split("\0")
    in_messages = collections.Counter()
    for body in messages[1::2]:
        for name, pattern in compiled.items():
            if name == "email address":
                continue  # commit trailers carry authors' addresses by design
            if any(not HARMLESS.search(m.group(0)) for m in pattern.finditer(body)):
                in_messages[name] += 1

    out = sys.stdout
    out.write(f"# History scan of {options.ref}\n\n")
    out.write(f"{checked} text file versions and {len(messages) // 2} commit messages scanned.\n\n")
    if not found and not in_messages:
        out.write("Nothing found.\n")
    for name in sorted(found):
        files = found[name]
        out.write(f"## {name}: {len(files)} paths\n\n")
        for path, (count, sample) in sorted(files.items(), key=lambda item: -item[1][0])[:40]:
            out.write(f"- `{path}`: {count} match(es) across its versions, e.g. `{sample}`\n")
        if len(files) > 40:
            out.write(f"- and {len(files) - 40} more paths\n")
        out.write("\n")
    if in_messages:
        out.write("## In commit messages\n\n")
        for name, count in in_messages.most_common():
            out.write(f"- {name}: {count} commit message(s)\n")


if __name__ == "__main__":
    main()
