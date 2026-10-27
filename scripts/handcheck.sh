#!/usr/bin/env bash
# Scripted hand checks: type, click and drag inside Qur through the
# QUR_DEBUG_ACTIONS driver (type:/key:/click:/drag:/state), then check the
# log lines and what landed on disk. Screenshots of a covered window are stale,
# so nothing here trusts a picture.
#
# usage: scripts/handcheck.sh [check ...]     (default: all; list with -l)
# env:   QUR_BIN (default target/release/qur, or $CARGO_TARGET_DIR/release/qur)
#        HC_DIR  scratch folder (default spikes/out/handcheck); every check gets
#                its own QUR_HOME and a throwaway git repo there.
# Needs a logged-in session (the window opens); no agent, no network.
set -uo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=${QUR_BIN:-${CARGO_TARGET_DIR:-$ROOT/target}/release/qur}
HC=${HC_DIR:-$ROOT/spikes/out/handcheck}
mkdir -p "$HC"
[ -x "$BIN" ] || { echo "no binary at $BIN (cargo build --release -p qur_app)"; exit 2; }

PASS=0; FAIL=0; LOG=""; REPO=""
git_() { git -C "$REPO" -c user.name=hc -c user.email=hc@localhost -c commit.gpgsign=false "$@"; }

# setup <name> [vim]: scratch QUR_HOME (vim on/off) and repo with a.txt.
setup() {
  local name=$1
  export QUR_HOME="$HC/$name/home"
  REPO="$HC/$name/repo"
  LOG="$HC/$name/qur.log"
  rm -rf "${HC:?}/$name"; mkdir -p "$QUR_HOME" "$REPO"
  printf '[keys]\nvim = %s\n' "${2:-false}" > "$QUR_HOME/config.toml"
  # A plain shell stands in for Claude Code as the Agent Terminal's agent.
  cat > "$QUR_HOME/agents.toml" <<'EOF'
[[agent]]
id = "claude-terminal"
label = "Test shell agent"
mode = "terminal"
command = "/bin/sh"
EOF
  printf 'one\ntwo\nthree\n' > "$REPO/a.txt"
  git_ init -q; git_ add -A; git_ commit -qm init
}

# drive <steps> [qur arg]: run Qur until the steps end (they should end in qur::Quit).
# An arg of "-" runs it with none.
drive() {
  local args=("${2:-$REPO}")
  [ "${2:-}" = "-" ] && args=()
  QUR_DEBUG_ACTIONS="$1" perl -e 'alarm shift; exec @ARGV' 90 "$BIN" ${args[@]+"${args[@]}"} </dev/null >"$LOG" 2>&1
  grep -hE '^(STATE|FLASH|PROMPT|WINDOW|QUR_DEBUG)' "$LOG" | sed 's/^/    /'
}

# ok <description> <command...>: one assertion.
ok() {
  local what=$1; shift
  if "$@" >/dev/null 2>&1; then echo "  ✓ $what"; PASS=$((PASS + 1)); else echo "  ✕ $what"; FAIL=$((FAIL + 1)); fi
}
has() { grep -qF -- "$1" "$LOG"; }
last_state() { grep '^STATE' "$LOG" | tail -1; }
state_has() { last_state | grep -qF -- "$1"; }

# --- checks -------------------------------------------------------------------

check_vim() {  # Vim keys in the code modal, saved with Cmd+S.
  setup vim true
  drive "wait:1500,state,type:jdd,type:Oinserted,key:escape,type:jofinal,key:escape,state,key:cmd-s,wait:300,state,qur::Quit" "$REPO/a.txt"
  ok "modal opened in NORMAL mode" grep -q 'code=a.txt@1:1 vim=NORMAL' "$LOG"
  ok "jdd, O, j, o edited the buffer" grep -qF 'text="one\ninserted\nthree\nfinal\n"' "$LOG"
  ok "Esc left insert mode (not the modal)" state_has 'vim=NORMAL'
  ok "Cmd+S wrote it to disk" cmp -s "$REPO/a.txt" <(printf 'one\ninserted\nthree\nfinal\n')
}

check_find_terminal() {  # typing in Find in Terminal selects the match.
  setup find_terminal
  drive "wait:1500,qur::NewTerminal,wait:1500,type:echo needle-42 haystack\n,wait:800,key:cmd-f,type:needle-\\\\d+,wait:300,state,key:escape,state,qur::Quit"
  ok "find box got the typed regex" grep -qF 'find="needle-\\d+" found=true' "$LOG"
  ok "the match is selected" grep -qF 'selection="needle-42"' "$LOG"
  ok "Esc closed the find box" state_has 'find=none'
}

check_rename() {  # the Rename box: select all, type, Enter; saved with the task.
  setup rename
  drive "wait:1500,qur::NewAgentTerminal,wait:1500,qur::RenameAgent,wait:300,state,key:cmd-a,type:my renamed agent,state,key:enter,wait:500,state,qur::Quit"
  ok "Rename box opened with the current name" grep -qF 'overlay=rename:"agent terminal 1"' "$LOG"
  ok "typed name in the box" grep -qF 'overlay=rename:"my renamed agent"' "$LOG"
  ok "Enter closed it and renamed the task" state_has 'overlay=none selected=task:root-1:"my renamed agent"'
  ok "saved with the task" grep -rqF 'my renamed agent' "$QUR_HOME/projects"
}

check_git_commit() {  # Git panel: Commit Staged… focuses the message; type; Cmd+Enter.
  setup git_commit
  echo change >> "$REPO/a.txt"
  drive "wait:1500,qur::GitPanel,wait:800,qur::GitStageAll,wait:800,qur::GitCommit,wait:300,type:typed commit message,state,key:cmd-enter,wait:1000,state,qur::Quit"
  ok "message typed into the field" grep -qF 'message="typed commit message"' "$LOG"
  ok "commit made with that message" test "$(git_ log -1 --format=%s)" = "typed commit message"
  ok "field cleared, nothing left staged" state_has 'message="" '
}

check_commit_message() {  # an agent in the project folder: the Git panel offers a message from its work.
  setup commit_message
  drive "wait:1500,qur::NewAgentTerminal,wait:1500,qur::Quit"
  echo four >> "$REPO/a.txt"; printf 'new\n' > "$REPO/b.txt"
  drive "wait:2000,qur::GitPanel,wait:1500,state,qur::GitStageAll,wait:800,qur::GitCommit,wait:1000,state,qur::Quit"
  ok "the empty commit box got a subject and a body" grep -qF 'message="Update 2 files" body="- a.txt (+1 -0)\n- b.txt (+1 -0)\n\n2 files changed, +2 -0"' "$LOG"
  ok "Commit used both" test "$(git_ log -1 --format=%B)" = "$(printf 'Update 2 files\n\n- a.txt (+1 -0)\n- b.txt (+1 -0)\n\n2 files changed, +2 -0')"
  ok "and cleared them" state_has 'message="" body="" '
}

check_merge_message() {  # Merge: the task's commit and the merge commit carry a message from its turns.
  setup merge_message
  local id wt base
  id=$(git_ rev-list --max-parents=0 HEAD | cut -c1-12); wt="$QUR_HOME/worktrees/$id/dark-mode"; base=$(git_ rev-parse HEAD)
  mkdir -p "$(dirname "$wt")"; git_ worktree add -q -b qur/dark-mode "$wt"
  printf 'one\ntwo\nTHREE\n' > "$wt/a.txt"; printf 'pub fn theme() {}\n' > "$wt/theme.rs"
  # An ACP task with one recorded turn; its agent never starts (it isn't sent anything).
  printf '[[agent]]\nid = "fake"\nlabel = "Fake"\nmode = "acp"\ncommand = "/usr/bin/false"\n' >> "$QUR_HOME/agents.toml"
  mkdir -p "$QUR_HOME/projects"
  printf '[[task]]\nid = "dark-mode"\nagent = "fake"\ntitle = "can you add a dark mode toggle please"\nbranch = "qur/dark-mode"\nworktree = "%s"\nbase = "%s"\ncreated = 1\nmode = "acp"\n\n[[task.turns]]\nn = 1\nstart = "x"\nend = "y"\nfiles = 2\nadded = 2\nremoved = 1\n' "$wt" "$base" > "$QUR_HOME/projects/$id.tasks.toml"
  QUR_DEBUG_PROMPT="1" drive "wait:2000,qur::NextAgent,wait:500,qur::Merge,wait:3000,qur::Quit"
  ok "the task's work was committed with the generated message" test "$(git_ log -1 --format=%B qur/dark-mode)" = "$(printf 'Add a dark mode toggle\n\n- a.txt (+1 -1)\n- theme.rs (+1 -0)\n\n2 files changed, +2 -1')"
  ok "the merge commit too, under Merge <branch>:" test "$(git_ log -1 --format=%B)" = "$(printf 'Merge qur/dark-mode: Add a dark mode toggle\n\n- a.txt (+1 -1)\n- theme.rs (+1 -0)\n\n2 files changed, +2 -1')"
  git_ worktree remove --force "$wt" >/dev/null 2>&1
}

esc_closed_menu() { grep '^STATE' "$LOG" | grep -A1 -F 'overlay=agent_menu:root-1' | sed -n 2p | grep -qF 'overlay=none'; }
check_rename_click() {  # right-click menu > Rename…, and double-click the button.
  setup rename_click
  drive "wait:1500,qur::NewAgentTerminal,wait:1500,rclick:@agent:tt-root-1,state,key:escape,wait:200,state,rclick:@agent:tt-root-1,click:@menu:Rename…,state,key:escape,click:@pane:Terminal:root,dclick:@agent:tt-root-1,state,key:cmd-a,type:by double click,key:enter,wait:300,state,qur::Quit"
  ok "right-click opened the agent menu" grep -qF 'overlay=agent_menu:root-1' "$LOG"
  ok "Esc closed the agent menu" esc_closed_menu
  ok "its Rename… opened the Rename box" grep -qF 'overlay=rename:"agent terminal 1"' "$LOG"
  ok "double-click opened it too, typed name kept" state_has 'overlay=none selected=task:root-1:"by double click"'
}

check_dock() {  # dock: + to add a tab, drag a tab to split, ×, drag a divider; saved.
  setup dock
  drive "wait:1500,qur::NewAgentTerminal,wait:1500,click:@plus:Terminal:root,wait:200,click:@add:Terminal:Live,wait:300,state,drag:@tab:Terminal:Live,@pane:Terminal:root~0.92~0.5,wait:300,state,click:@close:Terminal:Live,wait:300,state,click:@plus:Terminal:root,wait:200,click:@add:Terminal:Changes,wait:300,drag:@tab:Terminal:Changes,@pane:Terminal:root~0.5~0.9,wait:300,state,drag:@split:Terminal:root,700,300,wait:500,state,qur::Quit"
  ok "+ Live added a tab" grep -qF 'terminal=session,*live}' "$LOG"
  ok "dragging it to the right edge split the pane" grep -qF 'terminal=h0.500(session|live)}' "$LOG"
  ok "× closed it" grep -qF 'terminal=session}' "$LOG"
  ok "dragging Changes to the bottom edge split below" grep -qF 'terminal=v0.500(session|changes)}' "$LOG"
  local after; after=$(last_state | sed -nE 's/.*terminal=(v0\.[0-9]+\(session\|changes\))}.*/\1/p')
  ok "dragging the divider moved it (${after:-?})" test -n "$after" -a "$after" != "v0.500(session|changes)"
  ok "the arrangement is saved in config.toml" grep -qF "terminal = \"$after\"" "$QUR_HOME/config.toml"
}

check_cmd_click() {  # Cmd-click a path:line:col in a terminal opens it at that line.
  setup cmd_click
  drive "wait:1500,qur::NewTerminal,wait:1500,type:clear; echo a.txt:2:1\n,wait:1000,cmdclick:@term-cell:1~2.5~0.5,wait:500,state,qur::Quit"
  ok "the code modal opened a.txt at line 2" state_has 'code=a.txt@2:1'
}

check_git_stage() {  # Git panel: per-file + and −, and selecting a file shows its diff.
  setup git_stage
  echo change >> "$REPO/a.txt"
  drive "wait:1500,qur::GitPanel,wait:1000,state,click:@git-act:unstaged:a.txt,wait:800,state,click:@git-row:staged:a.txt,wait:800,state,click:@git-act:staged:a.txt,wait:800,state,qur::Quit"
  ok "starts with one unstaged file" grep -qF 'staged=0 unstaged=1' "$LOG"
  ok "+ staged it" grep -qF 'staged=1 unstaged=0' "$LOG"
  ok "clicking it showed its diff" grep -qF 'selected=staged:a.txt diff=[a.txt+1-0]' "$LOG"
  ok "− unstaged it again" state_has 'staged=0 unstaged=1'
  ok "the selection followed it to unstaged, with its diff" state_has 'selected=unstaged:a.txt diff=[a.txt+1-0]'
  ok "git agrees: nothing staged, a.txt modified" test "$(git_ status --porcelain)" = " M a.txt"
}

check_panels() {  # About and Accent Color open (and a click outside closes them); Run Doctor's sheet.
  setup panels
  QUR_DEBUG_PROMPT=0 drive "wait:1500,qur::About,wait:300,state,click:40,600,state,qur::AccentColor,wait:300,state,click:40,600,state,qur::RunDoctor,wait:4000,qur::Quit"
  ok "About opened" grep -qF 'overlay=about ' "$LOG"
  ok "Accent Color opened" grep -qF 'overlay=accent ' "$LOG"
  ok "a click outside closed each" test "$(grep -c '^STATE.*overlay=none ' "$LOG")" -eq 2
  ok "Run Doctor showed its sheet" grep -qE '^PROMPT (Doctor|.*doctor|.*good|.*problem|.*warning)' "$LOG"
}

check_gaps() {  # window gaps: toggle, a dock divider and the sidebar gap still drag, the popover saves.
  setup gaps
  drive "wait:1500,qur::ToggleGaps,wait:300,state,qur::NewAgentTerminal,wait:1500,click:@plus:Terminal:root,wait:200,click:@add:Terminal:Live,wait:300,drag:@tab:Terminal:Live,@pane:Terminal:root~0.92~0.5,wait:300,state,drag:@split:Terminal:root,700,300,wait:500,state,drag:@sidebar-gap,320,300,wait:300,state,qur::Gaps,wait:300,state,click:@gaps:slider0~0.999~0.5,wait:200,click:@gaps:rounded,wait:200,key:escape,wait:300,state,qur::ToggleGaps,wait:300,state,qur::Quit"
  ok "View > Toggle Gaps turned them on" grep -qF 'FLASH Gaps on' "$LOG"
  ok "gaps on in the state" grep -qF 'gaps={on=true gap=8' "$LOG"
  ok "a tab dragged to the edge split the dock with gaps on" grep -qF 'terminal=h0.500(session|live)}' "$LOG"
  local after; after=$(grep '^STATE' "$LOG" | sed -nE 's/.*terminal=(h0\.[0-9]+\(session\|live\))}.*/\1/p' | tail -1)
  ok "dragging the divider in the gap moved it (${after:-?})" test -n "$after" -a "$after" != "h0.500(session|live)"
  ok "the sidebar gap resized the sidebar" grep -qE 'sidebar_w=(29[1-9]|3[0-9][0-9])' "$LOG"
  ok "View > Appearance > Gaps… opened the popover" grep -qF 'overlay=gaps' "$LOG"
  ok "the slider and Rounded corners changed it live" grep -qF 'gaps={on=true gap=24 rounded=false' "$LOG"
  ok "Esc saved the popover's gap to config.toml" grep -qE '^gap = 24' "$QUR_HOME/config.toml"
  ok "and its Rounded corners" grep -qE '^rounded = false' "$QUR_HOME/config.toml"
  ok "Toggle Gaps turned them off and saved that" grep -qF 'gaps = false' "$QUR_HOME/config.toml"
}

check_gaps_terminal() {  # with gaps on, split shells are tiles and their divider still drags.
  setup gaps_terminal
  printf '[ui]\ngaps = true\n' >> "$QUR_HOME/config.toml"
  drive "wait:1500,qur::NewTerminal,wait:1500,key:cmd-d,wait:800,state,drag:@term-split:root,500,300,wait:300,state,qur::Quit"
  ok "Cmd+D split the shell" grep -qF 'sessions=2 active=1 find=none selection="" split=0.500' "$LOG"
  local after; after=$(last_state | sed -nE 's/.* split=([0-9.]+).*/\1/p')
  ok "dragging the gap between them moved the divider (${after:-?})" test -n "$after" -a "$after" != "0.500"
}

cfg_has() { grep -qE -- "$1" "$QUR_HOME/config.toml"; }
check_sidebar() {  # sidebar: drag a section header to reorder; auto-hide slides in at the edge; right side.
  setup sidebar
  drive "wait:1500,state,drag:@section:files,@section:git,wait:300,state,qur::AutoHideSidebar,wait:300,state,move:600,300,move:3,300,wait:300,state,move:700,300,wait:300,state,qur::SidebarOnRight,wait:300,move:600,300,move:@window~0.998~0.5,wait:300,state,qur::Quit"
  ok "dragging FILES onto SOURCE CONTROL moved it above" grep -qF 'sections=agents,terminals,files,git}' "$LOG"
  ok "and saved the order" cfg_has '^sidebar_sections = \["agents", "terminals", "files", "git"\]'
  ok "View > Auto-hide Sidebar hid it" grep -qF 'mode=auto side=left shown=false visible=false' "$LOG"
  ok "the mouse at the left edge slid it in" grep -qF 'mode=auto side=left shown=true visible=true' "$LOG"
  ok "and moving away hid it again" test "$(grep -c 'mode=auto side=left shown=false' "$LOG")" -ge 2
  ok "Sidebar on Right saved" cfg_has '^sidebar_side = "right"'
  ok "on the right, the right edge slides it in" grep -qF 'mode=auto side=right shown=true visible=true' "$LOG"
}

check_settings() {  # Cmd+, opens Settings; Vim, a theme and the opacity slider save live; Esc closes.
  setup settings
  drive "wait:1500,key:cmd-\\,,wait:300,state,click:@settings:nav:Editor,wait:200,click:@settings:vim:on,wait:300,state,click:@settings:nav:Appearance,wait:200,click:@settings:theme:1,wait:300,scroll:@settings:nav:Files~3~0.5,-500,wait:200,drag:@settings:slider:Opacity~0.95~0.5,@settings:slider:Opacity~0.1~0.5,wait:300,click:@settings:blur,wait:300,click:@settings:gaps:16,wait:300,click:@settings:sidebar-side:right,wait:300,state,click:@settings:sidebar:auto,wait:300,key:escape,wait:200,state,qur::Quit"
  ok "Cmd+, opened Settings (not config.toml)" grep -qF 'overlay=settings:Agents ' "$LOG"
  ok "the section list jumped to Editor" grep -qF 'overlay=settings:Editor ' "$LOG"
  ok "Vim on, saved at once" cfg_has '^vim = true'
  ok "a theme click saved it for all projects" cfg_has '^theme = "'
  ok "Gaps: Large saved at once" cfg_has '^gaps = true'
  ok "with its gap" cfg_has '^gap = 16'
  ok "the opacity slider saved its value" cfg_has '^opacity = 0\.6[0-9]*$'
  ok "Blur clicked off, saved" cfg_has '^blur = false'
  ok "Esc closed Settings" state_has 'overlay=none'
  ok "Sidebar side: Right saved at once" cfg_has '^sidebar_side = "right"'
  ok "and moved the sidebar there" grep -qF 'side=right shown=false visible=true' "$LOG"
  ok "Sidebar: Auto-hide saved" cfg_has '^sidebar = "auto"'
}

check_first_run() {  # QUR_DEBUG_SETUP=1: the first-run welcome; Done (or Skip for now) closes it.
  setup first_run
  QUR_DEBUG_SETUP=1 drive "wait:2500,state,click:@settings:done,wait:300,state,qur::Quit"
  ok "the welcome opened by itself" grep -qF 'overlay=settings:Agents ' "$LOG"
  ok "Done closed it" state_has 'overlay=none'
}

check_diff_review() {  # an agent's changes in the code modal: markers, Next/Previous Change, peek, Revert, Accept, undo.
  setup diff_review
  printf 'one\ntwo\nthree\nfour\nfive\n' > "$REPO/a.txt"; git_ commit -qam five
  # The second Agent Terminal gets a worktree; its shell plays the agent.
  drive "wait:1500,qur::NewAgentTerminal,wait:1000,qur::NewAgentTerminal,wait:2000,type:sed -i '' -e s/one/ONE/ -e /three/d a.txt; echo added >> a.txt\n,wait:1000,qur::QuickOpen,wait:400,type:a.txt,wait:300,key:enter,wait:1000,state,key:alt-f5,key:alt-f5,key:shift-alt-f5,key:alt-f3,wait:100,qur::RevertChange,wait:600,state,qur::AcceptChange,wait:100,state,key:cmd-z,wait:600,state,qur::Quit"
  local wt; wt=$(git_ worktree list --porcelain | sed -n 's/^worktree //p' | grep -v "^$REPO$" | head -1)
  ok "markers vs the task's base" grep -qF 'changes=[Modified@1 Removed@3 Added@5] peek=0 vs "the task'"'"'s base"' "$LOG"
  ok "Opt+F5 went to the removal" grep -qF 'CHANGE next line=3 ' "$LOG"
  ok "then to the addition" grep -qF 'CHANGE next line=5 ' "$LOG"
  ok "Shift+Opt+F5 went back" grep -qF 'CHANGE prev line=3 ' "$LOG"
  ok "Opt+F3 opened the peek on it" grep -qF 'CHANGE peek line=3 [Modified@1 Removed@3 Added@5] peek=2' "$LOG"
  ok "Revert put the removed line back; the peek moved on" grep -qF 'changes=[Modified@1 Added@6] peek=2' "$LOG"
  ok "in the buffer only" grep -qF 'dirty=true changes=[Modified@1 Added@6] peek=2 vs "the task'"'"'s base" format="UTF-8 · LF" text="ONE\ntwo\nthree\nfour\nfive\nadded\n"' "$LOG"
  ok "Accept hid the addition" grep -qF 'changes=[Modified@1] peek=0' "$LOG"
  ok "Cmd+Z undid the revert (the accepted change stays hidden)" state_has 'changes=[Modified@1 Removed@3] peek=0'
  ok "the worktree's file is the agent's" cmp -s "$wt/a.txt" <(printf 'ONE\ntwo\nfour\nfive\nadded\n')
  ok "its index untouched" git -C "$wt" diff --cached --quiet
}

check_diff_head() {  # your own file shows its changes vs HEAD; Revert Change in the main checkout.
  setup diff_head
  printf 'one\nTWO\nthree\nmore\n' > "$REPO/a.txt"
  drive "wait:1500,state,key:alt-f3,wait:100,state,key:escape,wait:100,state,qur::RevertChange,wait:600,state,qur::Quit" "$REPO/a.txt"
  ok "markers vs HEAD" grep -qF 'changes=[Modified@2 Added@4] peek=0 vs "HEAD"' "$LOG"
  ok "Opt+F3 went to the next change and opened its peek" grep -qF 'code=a.txt@2:1 vim=- dirty=false changes=[Modified@2 Added@4] peek=1' "$LOG"
  ok "Esc closed the peek, not the modal" grep -qF 'code=a.txt@2:1 vim=- dirty=false changes=[Modified@2 Added@4] peek=0' "$LOG"
  ok "Revert at the cursor's change only" state_has 'changes=[Added@4] peek=0 vs "HEAD" format="UTF-8 · LF" text="one\ntwo\nthree\nmore\n"'
  ok "nothing staged" git_ diff --cached --quiet
}

check_cleanup() {  # Clean Up Worktrees…: orphans preselected and removed; unmerged/dirty only after a second yes; a task's own kept.
  setup cleanup
  local W="$QUR_HOME/worktrees/$(git_ rev-list --max-parents=0 HEAD | cut -c1-12)"
  mkdir -p "$W" "$QUR_HOME/worktrees/ffffffffffff/stray"
  git_ worktree add -q -b qur/claude/orphan "$W/orphan"
  git_ worktree add -q -b qur/claude/work "$W/work"
  echo w > "$W/work/w.txt"; git -C "$W/work" add -A; git -C "$W/work" -c user.name=hc -c user.email=hc@localhost -c commit.gpgsign=false commit -qm work
  git_ worktree add -q -b qur/claude/wip "$W/wip"; echo edited > "$W/wip/a.txt"
  git_ worktree add -q -b qur/claude/gone "$W/gone"; rm -rf "${W:?}/gone"
  git_ branch qur/claude/left
  git_ update-ref refs/qur/turns/dead/1/start "$(git_ rev-parse 'HEAD^{tree}')"
  echo x > "$QUR_HOME/worktrees/ffffffffffff/stray/x.txt"
  touch -t 202601010000 "$W"/* "$QUR_HOME/worktrees/ffffffffffff/stray"  # old enough to preselect
  # Two Agent Terminals: the second works in its own (owned, brand-new) worktree.
  QUR_DEBUG_PROMPT="1,0" drive "wait:1500,qur::NewAgentTerminal,wait:800,qur::NewAgentTerminal,wait:1500,qur::CleanUpWorktrees,wait:2500,state,click:@cleanup:remove,wait:2500,state,click:@cleanup:row:qur/claude/work,click:@cleanup:row:qur/claude/wip,click:@cleanup:remove,wait:2000,state,click:@cleanup:remove,wait:2500,state,key:escape,state,qur::Quit"
  ok "orphans listed and preselected" grep -qE '^CLEANUP qur/claude/orphan \| no task · merged · clean \| .* \| picked' "$LOG"
  ok "the folder-gone worktree, the lone branch and dead turn refs too" test "$(grep -cE '^CLEANUP (qur/claude/gone \(folder gone\)|qur/claude/left \(no worktree\)|turn checkpoints of “dead”).*\| picked$' "$LOG")" -eq 3
  ok "unmerged, dirty and the unknown folder are not preselected" test "$(grep -cE '^CLEANUP (qur/claude/work \| 1 unmerged commit|qur/claude/wip \| uncommitted changes|stray \(unknown folder\)).*\| -$' "$LOG")" -ge 3
  ok "the live task's worktree shows its task and isn't selected" grep -qE '^CLEANUP .*\| task “.*\| -$' "$LOG"
  ok "the first Remove took the 4 orphans" has 'FLASH Removed 4,'
  ok "Keep Them left the unmerged and dirty ones" grep -qE '^PROMPT Remove 2 items with unsaved work\? -> 1' "$LOG"
  ok "Remove Them Too removed them" has 'FLASH Removed 2,'
  ok "git agrees: only the task's branch is left" test "$(git_ for-each-ref --format='%(refname)' refs/heads/qur refs/qur | tr '\n' ' ')" = "refs/heads/qur/claude/term-2 "
  ok "the stray folder stays (it asks first)" test -d "$QUR_HOME/worktrees/ffffffffffff/stray"
  ok "the project folder is untouched" test "$(git_ status --porcelain)" = ""
  ok "Esc closed it" state_has 'overlay=none'
}

# A plain folder (no git, outside any checkout: $HC may sit in one), and a
# state.toml whose most recent project is gone.
plain_and_recent() {
  PLAIN="${TMPDIR:-/tmp}/qur-hc-$1/plain"
  rm -rf "${PLAIN:?}"; mkdir -p "$PLAIN"; printf 'notes\n' > "$PLAIN/notes.txt"
  PLAIN=$(cd "$PLAIN" && pwd -P)
  printf 'recent_projects = ["%s", "%s", "%s"]\nsetup_done = true\n' "$HC/$1/gone" "$REPO" "$PLAIN" > "$QUR_HOME/state.toml"
}

check_empty_window() {  # no project to reopen: the empty window opens with a shell in ~, project commands off; a Settings theme saves.
  setup empty_window
  drive "wait:1500,state,qur::QuickOpen,qur::GitPanel,qur::FindInProject,wait:300,state,qur::NewTerminal,wait:1000,state,key:cmd-\\,,wait:300,click:@settings:nav:Appearance,wait:300,click:@settings:theme:1,wait:300,qur::Quit" -
  ok "the empty window opened (no project)" has 'WINDOW empty'
  ok "with a shell, and the sidebar showing it" grep -qE '^STATE.*project=none overlay=none selected=shell:1 .*sessions=1 .*visible=true' "$LOG"
  ok "Go to File, Git and Find in Project do nothing there" test "$(grep -c '^STATE.*project=none overlay=none selected=shell:1 .*git={none}' "$LOG")" -eq 2
  ok "New Terminal adds a second shell" state_has 'sessions=2'
  ok "and the empty window isn't a recent project" test -z "$(grep -F recent_projects "$QUR_HOME/state.toml" | grep -F /)"
  ok "a Settings theme click there saved it" cfg_has '^theme = "'
}

check_empty_recent() {  # opening a project that is gone: the empty window lists the others; a click opens one.
  setup empty_recent
  plain_and_recent empty_recent
  drive "wait:1500,marks,click:@home:recent:repo,wait:1500,state,qur::Quit" "$HC/empty_recent/gone"
  ok "says the project is gone" has "FLASH $HC/empty_recent/gone doesn't exist"
  ok "lists the projects still there" has 'BOUNDS home:recent:plain'
  ok "clicking one made the empty window that project's" state_has 'project=repo:repo'
  ok "without opening a second window" test "$(grep -c '^WINDOW ' "$LOG")" -eq 1
}

check_close_last() {  # closing the last project window leaves the empty window; closing that quits.
  setup close_last
  drive "wait:1500,qur::CloseWindow,wait:1000,state,qur::NewWindow,wait:500,qur::CloseWindow,wait:3000"
  ok "closing the project window opened the empty window" grep -qE '^WINDOW closed \(0 left\)' "$LOG"
  ok "the script followed it there" state_has 'project=none'
  ok "New Window reused it (no second empty window)" test "$(grep -c '^WINDOW empty' "$LOG")" -eq 1
  ok "closing the empty window quit Qur" has 'WINDOW closed empty (0 left)'
}

check_home_folder() {  # `qur ~` and a path that doesn't exist get the empty window.
  setup home_folder
  drive "wait:1200,state,qur::Quit" "$HOME"
  ok "qur ~ opened the empty window" has 'WINDOW empty'
  drive "wait:1200,qur::Quit" "$HC/home_folder/nowhere"
  ok "a missing path did too, and said so" has "FLASH $HC/home_folder/nowhere doesn't exist"
}

check_plain_folder() {  # a folder without git: a project without Source Control; agents offer git init.
  setup plain_folder
  plain_and_recent plain_folder
  export GIT_AUTHOR_NAME=hc GIT_AUTHOR_EMAIL=hc@localhost GIT_COMMITTER_NAME=hc GIT_COMMITTER_EMAIL=hc@localhost
  export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=commit.gpgsign GIT_CONFIG_VALUE_0=false
  QUR_DEBUG_OPEN="$PLAIN" QUR_DEBUG_PROMPT=0 drive "wait:1500,qur::NewAgentTerminal,wait:2000,state,qur::GitPanel,wait:300,state,qur::NewAgentTerminal,wait:4000,state,qur::GitPanel,wait:800,state,qur::Quit" -
  unset GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL GIT_CONFIG_COUNT GIT_CONFIG_KEY_0 GIT_CONFIG_VALUE_0
  ok "New Agent Terminal in the empty window asked for a folder" has "OPEN_PANEL $PLAIN"
  ok "and opened it as a project without git" has 'project=folder:plain'
  ok "its first Agent Terminal works in the folder" has 'selected=task:root-1:'
  ok "no Git panel there" grep -qE '^STATE.*project=folder:plain .*selected=task:root-1:.* git={none}' "$LOG"
  ok "a second one offered to initialize Git" has "PROMPT plain isn't a git repository -> 0"
  ok "which made a first commit of the folder" test -n "$(git -C "$PLAIN" log -1 --format=%s 2>/dev/null)"
  ok "then the agent started in its own worktree" has 'selected=task:term-2:'
  ok "and Git came on" state_has 'project=repo:plain'
}

ALL=(vim find_terminal rename rename_click dock cmd_click git_commit git_stage commit_message merge_message panels gaps gaps_terminal sidebar settings first_run diff_review diff_head cleanup empty_window empty_recent close_last home_folder plain_folder)
[ "${1:-}" = "-l" ] && { printf '%s\n' "${ALL[@]}"; exit 0; }
for c in "${@:-${ALL[@]}}"; do
  echo "$c"
  "check_$c"
done
echo "$PASS passed, $FAIL failed  (logs: $HC/<check>/qur.log)"
[ "$FAIL" -eq 0 ]
