#!/usr/bin/env python3
"""Generate contributors/contributors.json from this repo's real git history.

Why this exists: crates/ui-egui/build.rs bakes the About window's credits table from
contributors/contributors.json at compile time. The upstream project generates that file with an
internal script (craftrules/scripts/contributors.py) that isn't included in this trimmed repo, so
this is a small drop-in replacement that reads the same information straight from `git log` —
no network access, no external dependencies beyond git and Python's standard library.

It will NOT invent a name for you. It only lists people who actually have commits in `git log`
for this repository. So the normal order of operations is:

    1. git init / git remote add origin <your fork>   (if you haven't already)
    2. git add -A && git commit -m "..."               (commit as yourself: see note below)
    3. python3 contributors/contributors_gen.py
    4. edit contributors/people.toml to add your own display name (see that file's header)
    5. cargo build -p wordcraft-web --target wasm32-unknown-unknown   (bakes it into the binary)

Before step 2, make sure git knows who you are, or every commit (and this script) will credit
whatever name was last configured:

    git config user.name  "Your Name"
    git config user.email "you@example.com"

Usage:
    python3 contributors/contributors_gen.py [path-to-repo]

If path-to-repo is omitted, the repo containing this script is used.
"""

from __future__ import annotations

import json
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

FIELDS = ["lines_added", "lines_deleted", "binary_added", "binary_deleted", "commits", "prs"]


def run(repo: Path, *args: str) -> str:
    return subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True, text=True).stdout


def main() -> int:
    repo = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent

    try:
        head = run(repo, "rev-parse", "HEAD").strip()
    except subprocess.CalledProcessError:
        print("No commits yet in this repo — nothing to generate. Make at least one commit first.")
        return 1
    except FileNotFoundError:
        print("git was not found on PATH.")
        return 1

    branch = run(repo, "rev-parse", "--abbrev-ref", "HEAD").strip()
    head_date = run(repo, "show", "-s", "--format=%cI", "HEAD").strip()

    # %an = author name, %ae = author email, %aI = author date (ISO 8601), one record per commit,
    # followed by --numstat's per-file "added<TAB>deleted<TAB>path" lines (binary files show "-").
    log = run(repo, "log", "--numstat", "--no-merges", "--pretty=format:@@COMMIT@@%H\t%an\t%ae\t%aI")

    people: dict[str, dict] = defaultdict(lambda: {f: 0 for f in FIELDS} | {"first_commit": None, "last_commit": None})
    total_commits = 0
    current_key: str | None = None

    for line in log.splitlines():
        if line.startswith("@@COMMIT@@"):
            total_commits += 1
            _commit_hash, name, email, date = line.removeprefix("@@COMMIT@@").split("\t", 3)
            # Key by name (not email): the About window shows a GitHub-style "login", and most
            # people's git author name is close enough to their handle. Rename the key in the
            # JSON afterwards if you want it to exactly match your GitHub username.
            current_key = name.strip() or email.strip() or "unknown"
            p = people[current_key]
            p["commits"] += 1
            if p["first_commit"] is None or date < p["first_commit"]:
                p["first_commit"] = date
            if p["last_commit"] is None or date > p["last_commit"]:
                p["last_commit"] = date
        elif line.strip() and current_key is not None:
            parts = line.split("\t")
            if len(parts) != 3:
                continue
            added, deleted, _path = parts
            p = people[current_key]
            if added == "-" or deleted == "-":
                p["binary_added"] += 1 if added == "-" else 0
                p["binary_deleted"] += 1 if deleted == "-" else 0
            else:
                p["lines_added"] += int(added)
                p["lines_deleted"] += int(deleted)

    contributors = [
        {
            "login": login,
            "people_entry": False,  # set true by hand once you've added a matching entry to people.toml
            "github_name": None,
            "prs": data["prs"],
            "commits": data["commits"],
            "lines_added": data["lines_added"],
            "lines_deleted": data["lines_deleted"],
            "binary_added": data["binary_added"],
            "binary_deleted": data["binary_deleted"],
            "first_commit": data["first_commit"],
            "last_commit": data["last_commit"],
        }
        for login, data in sorted(people.items(), key=lambda kv: -kv[1]["commits"])
    ]

    out = {
        "schema": 1,
        "repo": "local",
        "branch": branch,
        "head": head,
        "head_date": head_date,
        "commits": total_commits,
        "unattributed_commits": 0,
        "contributors": contributors,
        "models": [],
    }

    out_path = Path(__file__).resolve().parent / "contributors.json"
    out_path.write_text(json.dumps(out, indent=2) + "\n")
    print(f"Wrote {out_path} — {len(contributors)} contributor(s) from {total_commits} commit(s).")
    if contributors:
        print("Now set people_entry: true for your own login above, and add a matching")
        print("[people.<login>] block in contributors/people.toml (see that file's header).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
