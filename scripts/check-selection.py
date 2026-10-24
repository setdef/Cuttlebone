#!/usr/bin/env python3
"""Shows that the milestone tier's selection of mutations (`scripts/mutate.py
--since REV`, by file) would have run every mutation that guards code a
milestone changed, for the milestones named here.

A mutation guards changed code if its snippet, in the milestone's last
commit, is on a line the milestone added or changed, or if the milestone
added the mutation or changed it (its snippet, replacement or tests): either
way, it had to be run again. Each milestone's mutations are the ones its own
scripts/mutate.py lists. Exits with status 1 if any such mutation isn't
selected.

Usage: scripts/check-selection.py [START..END ...]
"""
import importlib.util, os, re, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# Each milestone's first and last commits: the report before it, and its own.
MILESTONES = [
    ("milestone 3", "96855a1", "a928962"),
    ("milestone 4", "a928962", "4c4322a"),
    ("milestone 5", "4c4322a", "a19f986"),
]

# No scripts/__pycache__: tests/guardrails.rs reads every file in scripts/.
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("mutate", os.path.join(ROOT, "scripts", "mutate.py"))
mutate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mutate)


def git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True, check=True).stdout


def show(rev, path):
    r = subprocess.run(["git", "show", "%s:%s" % (rev, path)], cwd=ROOT, capture_output=True, text=True)
    if r.returncode != 0:
        raise OSError(path)
    return r.stdout


def mutations(rev):
    """The mutations scripts/mutate.py lists at `rev`: its `M`, without
    running the rest of it (which, in older versions, runs them)."""
    text = show(rev, "scripts/mutate.py")
    start = text.index("M = [")
    end = text.index("\n]\n", start) + 3
    scope = {}
    exec(text[start:end], scope)
    return scope["M"]


def changed_lines(start, end):
    """The lines of each file at `end` that the diff from `start` added or
    changed, and, for removed lines, the line where they were."""
    lines = {}
    path = None
    for line in git("diff", "-U0", start, end).splitlines():
        if line.startswith("+++ "):
            path = None if line == "+++ /dev/null" else line[6:]
        elif line.startswith("@@") and path:
            m = re.match(r"@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@", line)
            first, count = int(m.group(1)), int(m.group(2) or "1")
            lines.setdefault(path, set()).update(range(first, first + max(count, 1)))
    return lines


def guards(m, end, lines):
    """Whether a mutation's snippet is on a changed line at `end`."""
    name, path, old, new, test = m
    if path not in lines:
        return False
    try:
        text = show(end, path)
    except OSError:
        return False
    at = text.find(old)
    if at < 0:
        return False
    first = text.count("\n", 0, at) + 1
    last = first + old.count("\n")
    return any(l in lines[path] for l in range(first, last + 1))


def main(argv):
    ranges = [(r, *r.split("..")) for r in argv] or MILESTONES
    ok = True
    for label, start, end in ranges:
        before = {m[0]: m for m in mutations(start)}
        after = mutations(end)
        lines = changed_lines(start, end)
        changed = set(git("diff", "--name-only", start, end).split())
        chosen = {m[0] for m, _ in mutate.selected(after, changed, read=lambda p: show(end, p))}
        guarding = {}
        for m in after:
            if before.get(m[0]) != m:
                guarding[m[0]] = "added or changed by the milestone" if m[0] not in before else "changed by the milestone"
            elif guards(m, end, lines):
                guarding[m[0]] = "its snippet is in changed code"
        missed = sorted(set(guarding) - chosen)
        print("%s (%s..%s): %d mutations, %d guard changed code, %d selected by file, %d of those missed"
              % (label, start, end, len(after), len(guarding), len(chosen), len(missed)))
        for name in missed:
            print("  MISSED: %s (%s)" % (name, guarding[name]))
        ok &= not missed
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
