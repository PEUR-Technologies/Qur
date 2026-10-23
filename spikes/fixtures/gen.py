#!/usr/bin/env python3
"""Generate deterministic spike fixtures.

  big-repo/  5,000 files across nested folders (tree-view test)
  large.ts   10,000-line TypeScript file (editor test)
"""
import os
import random
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
rng = random.Random(42)

WORDS = ["agent", "tree", "buffer", "worktree", "merge", "peek", "glow", "rope",
         "cursor", "theme", "token", "session", "prompt", "diff", "branch", "panel"]
EXTS = [".ts", ".tsx", ".rs", ".py", ".json", ".md", ".css", ".toml"]


def big_repo(root, n_files=5000):
    count = 0
    while count < n_files:
        depth = rng.randint(1, 5)
        parts = [f"{rng.choice(WORDS)}_{rng.randint(0, 20)}" for _ in range(depth)]
        d = os.path.join(root, *parts)
        os.makedirs(d, exist_ok=True)
        for _ in range(rng.randint(1, 12)):
            if count >= n_files:
                break
            p = os.path.join(d, f"{rng.choice(WORDS)}_{count}{rng.choice(EXTS)}")
            with open(p, "w") as f:
                f.write(f"// file {count}\n")
            count += 1
    return count


def ts_block(i):
    w = rng.choice(WORDS)
    W = w.capitalize()
    return f'''/**
 * {W}Service #{i}: handles {w} lifecycle for the agent roster.
 */
export interface {W}Options{i} {{
  id: string;
  retries?: number;
  tags: ReadonlyArray<string>;
}}

export class {W}Service{i}<T extends {{ id: string }}> {{
  private readonly cache = new Map<string, T>();
  static readonly VERSION = "{i}.0.{rng.randint(0, 99)}";

  constructor(private opts: {W}Options{i}, private log = console) {{}}

  async load(id: string): Promise<T | undefined> {{
    if (this.cache.has(id)) return this.cache.get(id);
    const res = await fetch(`/api/{w}/${{id}}?n=${{this.opts.retries ?? 3}}`);
    // TODO: handle 404 separately from network errors
    if (!res.ok) throw new Error(`load failed: ${{res.status}}`);
    const value = (await res.json()) as T;
    this.cache.set(id, value);
    return value;
  }}

  filter(pred: (x: T) => boolean): T[] {{
    return [...this.cache.values()].filter(pred).sort((a, b) => a.id.localeCompare(b.id));
  }}
}}

export const default{W}{i}: {W}Options{i} = {{ id: "{w}-{i}", retries: {rng.randint(1, 9)}, tags: ["a", "b"] }};
'''


def large_ts(path, target_lines=10000):
    out, n, i = ["// Generated 10k-line TypeScript fixture for the Qur toolkit spike.\n", "\n"], 2, 0
    while n < target_lines:
        b = ts_block(i)
        out.append(b)
        n += b.count("\n")
        i += 1
    text = "".join(out)
    lines = text.split("\n")[:target_lines]
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")
    return len(lines)


if __name__ == "__main__":
    repo = os.path.join(HERE, "big-repo")
    if not os.path.exists(repo):
        print("files:", big_repo(repo))
    if not os.path.exists(os.path.join(repo, ".git")):
        # A git repo so Cmd+Shift+T can create task worktrees from it.
        import subprocess
        git = ["git", "-C", repo, "-c", "user.email=spike@qur.local", "-c", "user.name=qur spike"]
        subprocess.run(git[:3] + ["init", "-q", "-b", "main"], check=True)
        subprocess.run(git + ["add", "-A"], check=True)
        subprocess.run(git + ["commit", "-qm", "fixture"], check=True)
        print("git: initialized")
    print("lines:", large_ts(os.path.join(HERE, "large.ts")))
    sys.exit(0)
