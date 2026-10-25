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
drive() {
  QUR_DEBUG_ACTIONS="$1" perl -e 'alarm shift; exec @ARGV' 90 "$BIN" "${2:-$REPO}" </dev/null >"$LOG" 2>&1
  grep -hE '^(STATE|FLASH|PROMPT|QUR_DEBUG)' "$LOG" | sed 's/^/    /'
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
  drive "wait:1500,qur::NewAgentTerminal,wait:1500,click:@add:Terminal:Live,wait:300,state,drag:@tab:Terminal:Live,@pane:Terminal:root~0.92~0.5,wait:300,state,click:@close:Terminal:Live,wait:300,state,click:@add:Terminal:Changes,wait:300,drag:@tab:Terminal:Changes,@pane:Terminal:root~0.5~0.9,wait:300,state,drag:@split:Terminal:root,700,300,wait:500,state,qur::Quit"
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
  drive "wait:1500,qur::ToggleGaps,wait:300,state,qur::NewAgentTerminal,wait:1500,click:@add:Terminal:Live,wait:300,drag:@tab:Terminal:Live,@pane:Terminal:root~0.92~0.5,wait:300,state,drag:@split:Terminal:root,700,300,wait:500,state,drag:@sidebar-gap,320,300,wait:300,state,qur::Gaps,wait:300,state,click:@gaps:slider0~0.999~0.5,wait:200,click:@gaps:rounded,wait:200,key:escape,wait:300,state,qur::ToggleGaps,wait:300,state,qur::Quit"
  ok "View > Toggle Gaps turned them on" grep -qF 'FLASH Gaps on' "$LOG"
  ok "gaps on in the state" grep -qF 'gaps={on=true gap=8' "$LOG"
  ok "a tab dragged to the edge split the dock with gaps on" grep -qF 'terminal=h0.500(session|live)}' "$LOG"
  local after; after=$(grep '^STATE' "$LOG" | sed -nE 's/.*terminal=(h0\.[0-9]+\(session\|live\))}.*/\1/p' | tail -1)
  ok "dragging the divider in the gap moved it (${after:-?})" test -n "$after" -a "$after" != "h0.500(session|live)"
  ok "the sidebar gap resized the sidebar" grep -qE 'sidebar_w=(29[1-9]|3[0-9][0-9])' "$LOG"
  ok "Qur > Appearance > Gaps… opened the popover" grep -qF 'overlay=gaps' "$LOG"
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

ALL=(vim find_terminal rename rename_click dock cmd_click git_commit git_stage panels gaps gaps_terminal)
[ "${1:-}" = "-l" ] && { printf '%s\n' "${ALL[@]}"; exit 0; }
for c in "${@:-${ALL[@]}}"; do
  echo "$c"
  "check_$c"
done
echo "$PASS passed, $FAIL failed  (logs: $HC/<check>/qur.log)"
[ "$FAIL" -eq 0 ]
