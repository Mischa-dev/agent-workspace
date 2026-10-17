#!/usr/bin/env python3
"""A command-line harness for Agent Workspace (protocol aw-harness/1).

Register it in Settings > Workspace > Harnesses with an argv template such as
["/opt/aw-harnesses/script-harness.py"] for the demo, which writes NOTES.md,
or ["/opt/aw-harnesses/script-harness.py", "/usr/local/bin/my-agent", "--model", "{model}"]
to run your agent in the worktree with the prompt on stdin. See docs/harness-guide.md.
"""
import json, os, signal, subprocess, sys, threading


def emit(**event):
    print(json.dumps(event), flush=True)


# fd 3 holds the secrets (one JSON object, then EOF). Read it once, never print it.
secrets = {}
if os.environ.get("AGENT_WORKSPACE_SECRETS_FD"):
    with os.fdopen(int(os.environ["AGENT_WORKSPACE_SECRETS_FD"]), encoding="utf-8") as fd:
        secrets = json.load(fd)

turn = json.loads(sys.stdin.readline())
agent = sys.argv[1:]
child = None


def watch_stdin():  # answers and stop arrive here after the turn line
    for line in sys.stdin:
        if json.loads(line).get("type") == "stop" and child:
            os.killpg(child.pid, signal.SIGTERM)  # the agent and anything it started


threading.Thread(target=watch_stdin, daemon=True).start()
emit(type="log", text=f"secrets handed over: {len(secrets)}")  # a count, never values

if agent:
    emit(type="command", id="agent", command=" ".join(agent), status="started")
    child = subprocess.Popen(agent, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, start_new_session=True)
    output, _ = child.communicate(turn["prompt"])
    emit(type="command", id="agent", status="completed", exitCode=child.returncode, output=output[-4000:])
    if child.returncode != 0:
        emit(type="done", status="failed", message=f"The agent exited with {child.returncode}.")
        sys.exit(0)
else:
    existed = os.path.exists("NOTES.md")
    with open("NOTES.md", "a", encoding="utf-8") as notes:
        notes.write(f"\n## {turn['turnId']}\n\n{turn['prompt']}\n")
    emit(type="file", path="NOTES.md", change="modified" if existed else "added")

status = subprocess.run(["git", "status", "--porcelain"], capture_output=True, text=True).stdout
for line in status.splitlines() if agent else []:
    emit(type="file", path=line[3:], change="deleted" if "D" in line[:2] else "added" if line[:2] == "??" else "modified")
# Your agent's real token counts go here; this demo estimates them from the prompt.
emit(type="usage", model=turn.get("model") or "script", inputTokens=len(turn["prompt"]) // 4 + 1, outputTokens=20, cachedInputTokens=0)
emit(type="message", text="Done: " + ("the agent changed the worktree." if agent else "wrote the request to NOTES.md."))
emit(type="done", status="completed")
