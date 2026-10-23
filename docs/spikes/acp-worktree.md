# M0 spike: ACP + worktrees

Date: 2026-10-23. Machine: Apple M5, macOS 27.0. Code: `spikes/acp_spike/` (client), `spikes/worktree_spike.sh` (end-to-end run).
Raw logs of every message in both directions: `spikes/out/<agent>-<time>/acp.jsonl` (not committed).

## Adapters (verified on npm, 2026-10-23)

| Agent | Package (installed globally, pinned) | Binary | Old name (deprecated) |
|---|---|---|---|
| Claude Code | `@agentclientprotocol/claude-agent-acp@0.94.0` | `claude-agent-acp` | `@zed-industries/claude-code-acp` |
| Codex | `@agentclientprotocol/codex-acp@2.2.2` (+ `@openai/codex@0.163.0`) | `codex-acp` | `@zed-industries/codex-acp` |

Both adapters answer `initialize` with **`protocolVersion: 1`**, even when offered 2. ACP v2 exists as `2.0.0-alpha.9` (unreleased schema). **Qur targets ACP v1.**

`codex-acp` advertises `authMethods: api-key, chat-gpt`. Qur must only ever use `chat-gpt` (or the existing `codex login`).

## Client weight

| Option | Release binary | Crates in tree | Notes |
|---|---|---|---|
| Hand-rolled (serde_json + std threads), complete spike client | **548 KB** | 2 direct | what `acp_spike` is |
| `agent-client-protocol` 3.5.0 (types only, `schema::v1`) | 806 KB for a trivial decoder | 63 | full crate also pulls futures, futures-concurrency, tracing, uuid, async-process |

**Recommendation:** hand-roll the transport (newline-delimited JSON-RPC over stdio, one reader thread per agent) in `qur_acp`, and define only the message types Qur reads (`session/update` variants, permission request, prompt response) with serde. Re-check against `schema/v1/schema.json` when bumping adapters. This avoids a second async runtime (handoff section 6).

## Claude Code run (subscription login, no API keys in env)

Prompt: create `src/greet.ts`, add `sub()` to `src/math.ts`.

| Step | Time |
|---|---|
| `initialize` | 105 to 332 ms (first run cold) |
| `session/new` | 0.8 to 3.9 s (first run cold) |
| `session/prompt` to `end_turn` | 5.8 s |
| shutdown (close stdin, SIGTERM group) | 0.5 to 0.8 s, no SIGKILL needed |

Messages in one turn: 19 `agent_message_chunk`, 3 `tool_call` (1 read, 2 edit), 12 `tool_call_update`, 6 `usage_update`, 2 `available_commands_update`, 2 `session/request_permission`, 3 `_auth/status_update` (adapter extension notification, ignore).

`session/cancel` (Stop / Take over): sent right after the first `tool_call`; the prompt returned `stopReason: "cancelled"` 1.6 s after it started. Works.

Orphans: the adapter is spawned with `process_group(0)`; shutdown signals the whole group. After every run, `pgrep -g <pgid>` and a scan of the pre-shutdown descendant tree found **no survivors**.

## Mapping tool calls to heatmap and Live (Claude adapter)

Observed sequence for one edit:

```
t=5075  tool_call         kind=edit status=pending title="Preparing file…" locations=[]
t=5610  tool_call_update  title="Write src/greet.ts" locations=[{path}]       <- path arrives here
t=5962  tool_call_update  content=[diff]  (rawInput has the full new content)
t=5972  session/request_permission  toolCall{kind=edit, locations, content=[{type:diff, path, oldText, newText}]}
t=5985  tool_call_update  status=completed
```

Findings:
- `kind` is set on the initial `tool_call`, but **`locations` is empty there**; the path comes 0.5 to 0.9 s later in a `tool_call_update`. Qur must keep a `toolCallId -> {kind, paths, status}` map and apply heat when paths arrive.
- Status went `pending` -> `completed` with no `in_progress`. Treat "not completed/failed" as active.
- Read locations include `line`, which feeds Live's scroll position.
- Paths are absolute inside the worktree. Strip the worktree prefix to map to tree rows.
- The permission request carries the full diff (`oldText`/`newText`), so Live can preview the edit before the file is written.

| ACP signal | Heatmap state | Live |
|---|---|---|
| `session/request_permission` with `toolCall.locations` | needs-approval (outline) on each path | jump to file, show pending diff |
| `tool_call`/update, `kind=edit` (or `delete`, `move`), not completed, has locations | editing (pulse) | follow the path, cursor at the diff range |
| same, `kind=read` or `search` | reading (faint, fades 1.5 s after completed) | no follow (Live follows edits only) |
| `kind=execute` (shell) | nothing directly; the file watcher catches writes | unchanged |
| `status=completed` on an edit | drop to changed/created, recompute `+n -m` after 150 ms | cursor stays at last range |
| `plan` update | n/a | To-do view |
| `agent_message_chunk`, tool calls | n/a | Chat view |

Watcher events on the worktree (FSEvents) cover `execute`, formatters and codegen, as planned.

## Codex run

Deferred by the owner (2026-10-23: "no codex for now"). The adapter is installed and answers `initialize` (above); a full run needs `codex login`, then `spikes/worktree_spike.sh codex codex-acp`.

## Worktrees and merge

Run: `spikes/worktree_spike.sh claude claude-agent-acp` -> PASS.

- Worktree created **outside the repo** at `<QUR_HOME>/worktrees/<repo-id>/<slug>/` on branch `qur/<agent>/<slug>` in 49 ms. `<repo-id>` = first 12 hex chars of the root commit (stable across clones and renames).
- The agent edited only its worktree; the main checkout was untouched during the turn.
- Agents don't commit (and were told not to run git). **Qur commits the worktree's changes as `qur: <task>` when the user presses Merge.**

**Merge into a dirty main checkout.** The test main checkout had a staged change, an unstaged change and a stash entry.
- Plain `git merge` **refuses** when anything is staged ("Your local changes would be overwritten"), even if the merge doesn't touch that file.
- What works, and is what Qur will do:
  1. `git merge-tree --write-tree HEAD <task-branch>` computes the merge in memory and reports conflicts without touching anything.
  2. `git commit-tree <tree> -p HEAD -p <task-branch>` creates the merge commit.
  3. `git read-tree -m -u HEAD <merge>` is a two-way merge: it updates the files the merge changed and carries the user's staged and unstaged changes along.
  4. `git update-ref HEAD <merge> <old-head>` moves the branch, guarded by the old value.
- Result: the index diff, unstaged diff and stash list were **byte-identical before and after**, and the agent's files landed on the user's branch.
- If a file the user has dirty is also changed by the task, `read-tree` refuses (`Entry 'f.txt' not uptodate`). HEAD doesn't move and nothing is touched; Qur then asks the user to commit or stash first.
- If `merge-tree` reports conflicts, Qur asks for a clean checkout, then runs a real `git merge` so conflict markers land in files the editor can open (ours/theirs/both).
- Discard: `git worktree remove` + `git branch -d`, both verified.

Needs git >= 2.38 for `merge-tree --write-tree` (system git here is 2.54). `qur doctor` should check this.
