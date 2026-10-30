"""Times the lock hook and the commit check in generated repositories of three sizes.

Usage: python tests/bench.py <path to the lock binary> [runs]

Each number is the wall time of one lock process, from start to exit, as an agent or
git would run it. The script prints a Markdown table with the median and the 95th
percentile of each case.
"""
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

LOCK = str(Path(sys.argv[1]).resolve())
RUNS = int(sys.argv[2]) if len(sys.argv) > 2 else 40
SIZES = [("10 files, 1 lock", 10, 1), ("1,000 files, 30 locks", 1_000, 30),
         ("10,000 files, 300 locks", 10_000, 300)]
BODY = "\n".join(f"export const line{n} = {n};" for n in range(60))


def git(repo, *args):
    subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True)


def build(root, files, locks):
    repo = Path(root)
    git(repo, "init", "-q", "-b", "main")
    git(repo, "config", "user.name", "bench")
    git(repo, "config", "user.email", "bench@example.com")
    git(repo, "config", "core.autocrlf", "false")
    for n in range(files):
        path = repo / "src" / f"dir{n % 50}" / f"file{n}.ts"
        path.parent.mkdir(parents=True, exist_ok=True)
        text = BODY
        if n < locks:
            text = (f"{BODY}\n/* LOCKED: copy-{n} START */\n"
                    f'export const tagline{n} = "Pay for what you ship.";\n'
                    f"/* LOCKED: copy-{n} END */\n")
        path.write_text(text + "\n", newline="\n")
    git(repo, "add", "-A")
    git(repo, "commit", "-qm", "init")
    return repo


def time_run(args, payload, cwd):
    data = json.dumps(payload).encode() if payload is not None else None
    start = time.perf_counter()
    subprocess.run(args, input=data, cwd=cwd, capture_output=True)
    return (time.perf_counter() - start) * 1000


def measure(args, payload, cwd):
    for _ in range(3):
        time_run(args, payload, cwd)
    samples = sorted(time_run(args, payload, cwd) for _ in range(RUNS))
    return statistics.median(samples), samples[int(len(samples) * 0.95) - 1]


def edit(repo, path, old, new):
    return {"cwd": str(repo), "session_id": "bench", "hook_event_name": "PreToolUse",
            "tool_name": "Edit", "tool_input": {"file_path": str(repo / path),
                                                "old_string": old, "new_string": new}}


def shell(repo, event):
    return {"cwd": str(repo), "session_id": "bench", "hook_event_name": event,
            "tool_name": "Bash", "tool_input": {"command": "true"}}


def cases(repo):
    hook = [LOCK, "hook", "claude"]
    locked = "src/dir0/file0.ts"
    plain = f"src/dir1/file{1 if len(list((repo / 'src' / 'dir1').iterdir())) else 0}.ts"
    return [
        ("Edit to a file without locks", hook,
         edit(repo, plain, "line5 = 5;", "line5 = 6;")),
        ("Edit outside a locked region", hook,
         edit(repo, locked, "line5 = 5;", "line5 = 6;")),
        ("Edit inside a locked region (refused)", hook,
         edit(repo, locked, "Pay for what you ship.", "Ship faster.")),
        ("Before a shell command", hook, shell(repo, "PreToolUse")),
        ("After a shell command", hook, shell(repo, "PostToolUse")),
        ("Commit check", [LOCK, "check", "--staged", "--record"], None),
    ]


def main():
    size = os.path.getsize(LOCK)
    print(f"lock binary: {size / 1024:.0f} KB ({sys.platform}), {RUNS} runs per case\n")
    base = measure([LOCK, "--version"], None, None)
    print(f"Process start (`lock --version`): median {base[0]:.1f} ms, p95 {base[1]:.1f} ms\n")
    results = {}
    names = []
    for label, files, locks in SIZES:
        root = tempfile.mkdtemp(prefix="lock-bench-")
        try:
            repo = build(root, files, locks)
            for name, args, payload in cases(repo):
                if name not in names:
                    names.append(name)
                if name == "After a shell command":
                    # Each check after a command needs the copy taken before it.
                    samples = []
                    for _ in range(RUNS + 3):
                        time_run(args, shell(repo, "PreToolUse"), repo)
                        samples.append(time_run(args, payload, repo))
                    samples = sorted(samples[3:])
                    results[(name, label)] = (statistics.median(samples),
                                              samples[int(len(samples) * 0.95) - 1])
                else:
                    results[(name, label)] = measure(args, payload, repo)
        finally:
            # git marks its object files read-only, which stops a plain delete on Windows.
            for folder, _, files in os.walk(root):
                for name in files:
                    os.chmod(os.path.join(folder, name), 0o600)
            shutil.rmtree(root, ignore_errors=True)
    print("| Case | " + " | ".join(label for label, _, _ in SIZES) + " |")
    print("| --- |" + " --- |" * len(SIZES))
    for name in names:
        cells = [f"{results[(name, label)][0]:.0f} ms (p95 {results[(name, label)][1]:.0f})"
                 for label, _, _ in SIZES]
        print(f"| {name} | " + " | ".join(cells) + " |")


main()
