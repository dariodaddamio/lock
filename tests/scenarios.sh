#!/usr/bin/env bash
# Scenario checks: runs the lock binary against hook payloads and git states in a throwaway repository.
set -u
LOCK="${LOCK:?set LOCK to the lock binary}"
export PATH="$(cd "$(dirname "$LOCK")" && pwd):$PATH"
WORK="$(mktemp -d)"
# The person's own Claude Code settings must not change what the scenarios expect.
export CLAUDE_CONFIG_DIR="$WORK/.claude-config"
export CODEX_HOME="$WORK/.codex-home"
cd "$WORK" || exit 1
sha() { if command -v sha256sum >/dev/null; then sha256sum; else shasum -a 256; fi; }
edit() { perl -pi -e "$1" "$2"; }
ROOT="$(pwd -W 2>/dev/null || pwd)"
git init -q -b main; git config user.name tester; git config user.email t@example.com

cat > CHANGELOG.md <<'EOF'
# Changelog

<!-- LOCKED: history START - released history is immutable -->
## 1.0.0 - October 30, 2026

Initial release.
<!-- LOCKED: history END -->

```md
<!-- LOCKED: example START -->
not a lock
```

## Unreleased
EOF
mkdir -p src
cat > src/app.ts <<'EOF'
export const free = 1;
/* LOCKED: tagline START */
export const tagline = "Pay for what you ship.";
/* LOCKED: tagline END */
EOF
echo '{"name":"catalog"}' > catalog.json
mkdir -p .locks; echo '{ "files": ["catalog.json"] }' > .locks/files.json
git add -A; git commit -qm init

pass=0; fail=0
expect() { # name, expected (allow|deny), payload
  out="$(printf '%s' "$3" | "$LOCK" hook claude 2>"$WORK/.stderr")"; code=$?
  if [ -n "$out" ]; then got=deny; else got=allow; fi
  [ $code -eq 2 ] && got="error"
  if [ "$got" = "$2" ]; then pass=$((pass+1)); echo "ok   $1"; else fail=$((fail+1)); echo "FAIL $1 (got $got) $out"; cat "$WORK/.stderr"; echo "input: $3"; fi
}
j() { printf '%s' "$1" | sed "s#ROOT#${ROOT//\\/\\\\}#g"; }

expect "edit outside region" allow "$(j '{"cwd":"ROOT","tool_name":"Edit","tool_input":{"file_path":"ROOT/src/app.ts","old_string":"free = 1","new_string":"free = 2"}}')"
expect "edit inside region" deny "$(j '{"cwd":"ROOT","tool_name":"Edit","tool_input":{"file_path":"ROOT/src/app.ts","old_string":"Pay for what you ship.","new_string":"Ship."}}')"
expect "edit removes END marker" deny "$(j '{"cwd":"ROOT","tool_name":"Edit","tool_input":{"file_path":"ROOT/src/app.ts","old_string":"/* LOCKED: tagline END */\n","new_string":""}}')"
expect "relative path edit in region" deny "$(j '{"cwd":"ROOT/src","tool_name":"Edit","tool_input":{"file_path":"app.ts","old_string":"Pay for","new_string":"Pay"}}')"
expect "fuzzy old text on locked file" deny "$(j '{"cwd":"ROOT","tool_name":"Edit","tool_input":{"file_path":"ROOT/src/app.ts","old_string":"not in file","new_string":"x"}}')"
expect "markdown region edit" deny "$(j '{"cwd":"ROOT","tool_name":"Edit","tool_input":{"file_path":"ROOT/CHANGELOG.md","old_string":"Initial release.","new_string":"First release."}}')"
expect "markdown outside region" allow "$(j '{"cwd":"ROOT","tool_name":"Edit","tool_input":{"file_path":"ROOT/CHANGELOG.md","old_string":"## Unreleased","new_string":"## Unreleased\n\nNew."}}')"
expect "fenced example is not a lock" allow "$(j '{"cwd":"ROOT","tool_name":"Edit","tool_input":{"file_path":"ROOT/CHANGELOG.md","old_string":"not a lock","new_string":"still not"}}')"
expect "write whole file over region" deny "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"ROOT/src/app.ts","content":"export const free = 1;\n"}}')"
expect "multiedit touching region" deny "$(j '{"cwd":"ROOT","tool_name":"MultiEdit","tool_input":{"file_path":"ROOT/src/app.ts","edits":[{"old_string":"free = 1","new_string":"free = 3"},{"old_string":"ship.","new_string":"pay."}]}}')"
expect "whole-file lock write" deny "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"ROOT/catalog.json","content":"{}"}}')"
expect "write approval record" deny "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"ROOT/.locks/approvals/x.json","content":"{}"}}')"
expect "add file lock" allow "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"ROOT/.locks/files.json","content":"{ \u0022files\u0022: [\u0022catalog.json\u0022\u002c \u0022other.json\u0022] }"}}')"
expect "drop file lock" deny "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"ROOT/.locks/files.json","content":"{ \u0022files\u0022: [] }"}}')"
expect "new lock added by agent" allow "$(j '{"cwd":"ROOT","tool_name":"Edit","tool_input":{"file_path":"ROOT/src/app.ts","old_string":"export const free = 1;","new_string":"// LOCKED: free START\nexport const free = 1;\n// LOCKED: free END"}}')"
expect "unrelated file" allow "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"ROOT/src/new.ts","content":"x"}}')"
expect "outside repo" allow "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"C:/elsewhere/a.ts","content":"x"}}')"
expect "notebook on locked file" deny "$(j '{"cwd":"ROOT","tool_name":"NotebookEdit","tool_input":{"notebook_path":"ROOT/catalog.json","new_source":"x"}}')"
expect "shell approve" deny "$(j '{"cwd":"ROOT","tool_name":"Bash","tool_input":{"command":"lock approve tagline"}}')"
expect "shell uninstall" deny "$(j '{"cwd":"ROOT","tool_name":"Bash","tool_input":{"command":"lock uninstall"}}')"
expect "shell command runs" allow "$(j '{"cwd":"ROOT","tool_name":"Bash","tool_input":{"command":"echo x > CHANGELOG.md"}}')"
expect "remove lock hook from settings" deny "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"ROOT/.claude/settings.json","content":"{\u0022disableAllHooks\u0022: true}"}}')"
expect "patch on locked file" deny "$(j '{"cwd":"ROOT","tool_name":"ApplyPatch","tool_input":{"patch":"*** Begin Patch\n*** Update File: src/app.ts\n@@\n-a\n+b\n*** End Patch"}}')"

# Codex shapes: patches are replayed, so a patch outside a region passes.
expect "codex patch outside region" allow "$(j '{"cwd":"ROOT","tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Update File: src/app.ts\n@@\n-export const free = 1;\n+export const free = 9;\n*** End Patch"}}')"
expect "codex patch inside region" deny "$(j '{"cwd":"ROOT","tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Update File: src/app.ts\n@@\n /* LOCKED: tagline START */\n-export const tagline = \"Pay for what you ship.\";\n+export const tagline = \"Ship.\";\n*** End Patch"}}')"
expect "codex patch deletes locked file" deny "$(j '{"cwd":"ROOT","tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Delete File: CHANGELOG.md\n*** End Patch"}}')"
expect "codex shell apply_patch" deny "$(j '{"cwd":"ROOT","tool_name":"shell","tool_input":{"command":["bash","-lc","apply_patch <<EOF\n*** Begin Patch\n*** Update File: CHANGELOG.md\n@@\n-Initial release.\n+Changed.\n*** End Patch\nEOF"]}}')"
expect "codex shell array runs" allow "$(j '{"cwd":"ROOT","tool_name":"exec_command","tool_input":{"cmd":["sed","-i","s/Pay/Ship/","src/app.ts"]}}')"

# Cursor shapes
cur="$(printf '%s' "$(j '{"cwd":"ROOT","command":"lock approve tagline"}')" | "$LOCK" hook cursor)"
case "$cur" in *'"permission":"deny"'*) pass=$((pass+1)); echo "ok   cursor shell approve deny";; *) fail=$((fail+1)); echo "FAIL cursor shell approve deny $cur";; esac
# The commands above only ran the check before them; the end of the turn clears their copies.
printf '%s' "$(j '{"cwd":"ROOT","hook_event_name":"Stop"}')" | "$LOCK" hook claude >/dev/null
cur="$(printf '%s' "$(j '{"cwd":"ROOT","tool_name":"StrReplace","tool_input":{"file_path":"ROOT/src/app.ts","old_string":"free = 1","new_string":"free = 2"}}')" | "$LOCK" hook cursor)"
case "$cur" in *'"permission":"allow"'*) pass=$((pass+1)); echo "ok   cursor edit allow";; *) fail=$((fail+1)); echo "FAIL cursor edit allow $cur";; esac

# A denied edit leaves a pending proposal for the person to review.
ls "$(git rev-parse --git-dir)/lock/pending" | grep -q json && { pass=$((pass+1)); echo "ok   pending proposal saved"; } || { fail=$((fail+1)); echo "FAIL pending proposal saved"; }

# Approval record (as `lock approve` would write it) lets the exact retry through, and nothing else.
before=$(printf '%s' "$(sed -n 2,4p src/app.ts)" | sha | cut -c1-64)
after=$(printf '/* LOCKED: tagline START */\nexport const tagline = "Ship.";\n/* LOCKED: tagline END */' | sha | cut -c1-64)
mkdir -p .locks/approvals
printf '{"version":1,"path":"src/app.ts","id":"tagline","before":"%s","after":"%s"}\n' "$before" "$after" > .locks/approvals/tagline.test.json
expect "approved exact retry" allow "$(j '{"cwd":"ROOT","tool_name":"Edit","tool_input":{"file_path":"ROOT/src/app.ts","old_string":"Pay for what you ship.","new_string":"Ship."}}')"
expect "different change still blocked" deny "$(j '{"cwd":"ROOT","tool_name":"Edit","tool_input":{"file_path":"ROOT/src/app.ts","old_string":"Pay for what you ship.","new_string":"Other."}}')"

# Git backstop: apply the approved change, commit check passes; an unapproved change fails.
edit 's/Pay for what you ship\./Ship./' src/app.ts
"$LOCK" check >/dev/null && { pass=$((pass+1)); echo "ok   check passes with approval"; } || { fail=$((fail+1)); echo "FAIL check passes with approval"; }
edit 's/Initial release\./Changed./' CHANGELOG.md
"$LOCK" check 2>/dev/null && { fail=$((fail+1)); echo "FAIL check catches unapproved md"; } || { pass=$((pass+1)); echo "ok   check catches unapproved md"; }
git checkout -q CHANGELOG.md
echo '{"name":"changed"}' > catalog.json
"$LOCK" check 2>/dev/null && { fail=$((fail+1)); echo "FAIL check catches whole-file"; } || { pass=$((pass+1)); echo "ok   check catches whole-file"; }
git checkout -q catalog.json
git add -A && "$LOCK" install --git >/dev/null && git commit -qm "approved tagline" && { pass=$((pass+1)); echo "ok   commit with approval"; } || { fail=$((fail+1)); echo "FAIL commit with approval"; }
edit 's/Initial release\./Sneaky./' CHANGELOG.md; git add -A
git commit -qm sneaky --no-verify
"$LOCK" check --base HEAD~1 2>/dev/null && { fail=$((fail+1)); echo "FAIL ci base catches --no-verify"; } || { pass=$((pass+1)); echo "ok   ci base catches --no-verify"; }
echo y | "$LOCK" approve 2>/dev/null && { fail=$((fail+1)); echo "FAIL approve refuses piped input"; } || { pass=$((pass+1)); echo "ok   approve refuses piped input"; }
echo y | "$LOCK" uninstall 2>/dev/null && { fail=$((fail+1)); echo "FAIL uninstall refuses piped input"; } || { pass=$((pass+1)); echo "ok   uninstall refuses piped input"; }

# Commands run, and lock checks what they changed afterwards.
check() { if eval "$2"; then pass=$((pass+1)); echo "ok   $1"; else fail=$((fail+1)); echo "FAIL $1 $out"; fi; }
hook() { printf '{"cwd":"%s","session_id":"s1","hook_event_name":"%s","tool_name":"Bash","tool_input":{"command":"%s"}}' "$ROOT" "$1" "$2" | "$LOCK" hook claude; }
agent() { out="$(hook PreToolUse "$1")"; [ -z "$out" ] || return 1; sh -c "$1" >/dev/null 2>&1; out="$(hook PostToolUse "$1")"; }
"$LOCK" install --claude >/dev/null; git add -A; git commit -qm hooks
mkdir -p tools
cat > tools/rewrite.sh <<'EOF'
perl -pi -e 's/"Ship\."/"Ship again."/; s/free = 1/free = 5/' src/app.ts
EOF
echo 'echo "{}" > .locks/approvals/forged.json' > tools/forge.sh
echo 'echo "{}" > .claude/settings.json' > tools/unhook.sh
cat > tools/commit.sh <<'EOF'
perl -pi -e 's/Sneaky\./Agent./' CHANGELOG.md && git commit -qam agent
EOF

agent "sh tools/rewrite.sh"
check "script change to a locked region is put back" 'grep -q "tagline = \"Ship.\";" src/app.ts && grep -q "free = 5" src/app.ts && case "$out" in *"put it back"*) true;; *) false;; esac'
agent "sh tools/forge.sh"
check "script that forges an approval is undone" '[ ! -e .locks/approvals/forged.json ] && case "$out" in *forged.json*) true;; *) false;; esac'
agent "sh tools/unhook.sh"
check "script that removes the hook is undone" 'grep -q "lock hook" .claude/settings.json'
edit 's/Sneaky\./Hand./' CHANGELOG.md
agent "git checkout -q -- CHANGELOG.md"
check "checkout back to HEAD is allowed" '[ -z "$out" ] && grep -q "Sneaky." CHANGELOG.md'
before=$(printf '/* LOCKED: tagline START */\nexport const tagline = "Ship.";\n/* LOCKED: tagline END */' | sha | cut -c1-64)
after=$(printf '/* LOCKED: tagline START */\nexport const tagline = "Ship again.";\n/* LOCKED: tagline END */' | sha | cut -c1-64)
printf '{"version":1,"path":"src/app.ts","id":"tagline","before":"%s","after":"%s"}\n' "$before" "$after" > .locks/approvals/again.json
agent "sh tools/rewrite.sh"
check "approved script change is kept" '[ -z "$out" ] && grep -q "Ship again." src/app.ts'
git checkout -q -- src/app.ts; rm .locks/approvals/again.json
head=$(git rev-parse HEAD)
agent "sh tools/commit.sh"
check "agent commit of a locked change is stopped" '[ "$(git rev-parse HEAD)" = "$head" ] && grep -q "Sneaky." CHANGELOG.md'
git reset -q
# A locked change needs an approval to be committed, even one made by hand, because
# a change that no hook saw could come from an agent without hooks.
approve_region() { # path, id: records the change from HEAD to the working tree, as `lock approve` would
  region() { sed -n "/LOCKED: $2 START/,/LOCKED: $2 END/p" | tr -d '\r'; }
  from=$(printf '%s' "$(git show "HEAD:$1" | region "$1" "$2")" | sha | cut -c1-64)
  to=$(printf '%s' "$(region "$1" "$2" < "$1")" | sha | cut -c1-64)
  printf '{"version":1,"path":"%s","id":"%s","before":"%s","after":"%s"}\n' "$1" "$2" "$from" "$to" > ".locks/approvals/$2.$to.json"
}
edit 's/Sneaky./By hand./' CHANGELOG.md
head=$(git rev-parse HEAD)
git commit -qam "hand edit" 2>/dev/null
check "unapproved hand edit is refused at commit" '[ "$(git rev-parse HEAD)" = "$head" ]'
approve_region CHANGELOG.md history
git add -A; git commit -qm "hand edit" 2>/dev/null
check "approved hand edit commits" '[ "$(git log -1 --format=%s)" = "hand edit" ]'
edit 's/By hand\./By hand again./' CHANGELOG.md
agent "git commit -qam agent-commits-hand-edit"
check "agent commit of an unapproved hand edit is refused" '[ "$(git log -1 --format=%s)" = "hand edit" ] && grep -q "By hand again." CHANGELOG.md'
git checkout -q CHANGELOG.md
hook PreToolUse "echo never-ran" >/dev/null
printf '{"cwd":"%s","session_id":"s1","hook_event_name":"Stop"}' "$ROOT" | "$LOCK" hook claude >/dev/null
check "turn end clears the copies" '[ -z "$(ls "$(git rev-parse --git-dir)/lock/runs" 2>/dev/null)" ]'

# Tools that lock cannot read, such as MCP tools, are checked by what they changed.
tool() { printf '{"cwd":"%s","session_id":"s3","hook_event_name":"%s","tool_name":"mcp__files__write","tool_use_id":"m1","tool_input":{"note":"x"}}' "$ROOT" "$1" | "$LOCK" hook claude; }
out="$(tool PreToolUse)"
edit 's/tagline = "[^"]*"/tagline = "From MCP."/' src/app.ts
out="$(tool PostToolUse)"
check "an MCP tool's change to locked text is put back" '! grep -q "From MCP." src/app.ts && case "$out" in *"put it back"*) true;; *) false;; esac'

# A command that runs in the background is checked when the turn ends.
bg() { printf '{"cwd":"%s","session_id":"s4","hook_event_name":"%s","tool_name":"Bash","tool_input":{"command":"sh tools/cursor.sh","run_in_background":true}}' "$ROOT" "$1" | "$LOCK" hook claude; }
bg PreToolUse >/dev/null
out="$(bg PostToolUse)"
edit 's/tagline = "[^"]*"/tagline = "Background."/' src/app.ts
stop="$(printf '{"cwd":"%s","session_id":"s4","hook_event_name":"Stop"}' "$ROOT" | "$LOCK" hook claude 2>/dev/null)"
check "a background command is checked when the turn ends" '[ -z "$out" ] && ! grep -q "Background." src/app.ts && case "$stop" in *systemMessage*"put it back"*) true;; *) false;; esac'

# Settings that decide whether lock runs: agent settings at home, and core.hooksPath.
mkdir -p "$CLAUDE_CONFIG_DIR"
CFG="$(cd "$CLAUDE_CONFIG_DIR" && (pwd -W 2>/dev/null || pwd))"
expect "agent cannot turn Claude Code hooks off in user settings" deny "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"'"$CFG"'/settings.json","content":"{\u0022disableAllHooks\u0022: true}"}}')"
mkdir -p "$CODEX_HOME"; CDX="$(cd "$CODEX_HOME" && (pwd -W 2>/dev/null || pwd))"
expect "agent cannot turn Codex hooks off" deny "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"'"$CDX"'/config.toml","content":"[features]\nhooks = false\n"}}')"
expect "agent may change other Codex settings" allow "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"'"$CDX"'/config.toml","content":"model = 1\n"}}')"
expect "agent cannot move the git hooks by editing .git/config" deny "$(j '{"cwd":"ROOT","tool_name":"Edit","tool_input":{"file_path":"ROOT/.git/config","old_string":"[core]","new_string":"[core]\n\thooksPath = /dev/null"}}')"
agent "git config core.hooksPath /dev/null"
check "a command that moves the git hooks is undone" '[ -z "$(git config --local --get core.hooksPath)" ] && case "$out" in *core.hooksPath*) true;; *) false;; esac'

# Claude Code asks the person in its permission prompt, and lock records a yes after the edit.
call() { printf '{"cwd":"%s","session_id":"s2","permission_mode":"%s","hook_event_name":"%s","tool_use_id":"%s","tool_name":"Edit","tool_input":{"file_path":"%s/src/app.ts","old_string":"%s","new_string":"%s"}}' "$ROOT" "$1" "$2" "$3" "$ROOT" "$4" "$5" | "$LOCK" hook "${6:-claude}"; }
out="$(call default PreToolUse t1 'Ship.' 'Ship today.')"
check "first locked edit is refused in the chat" 'case "$out" in *\"deny\"*"retry the same edit"*) true;; *) false;; esac'
out="$(call default PreToolUse t1 'Ship.' 'Ship today.')"
check "the same edit again asks in the prompt" 'case "$out" in *\"ask\"*tagline*) true;; *) false;; esac'
out="$(call default PreToolUse t1 'Ship.' 'Ship differently.')"
check "a different edit is refused first" 'case "$out" in *\"deny\"*) true;; *) false;; esac'
call acceptEdits PreToolUse t1 'Ship.' 'Ship today.' >/dev/null
out="$(call acceptEdits PreToolUse t1 'Ship.' 'Ship today.')"
check "accept-edits mode still asks" 'case "$out" in *\"ask\"*) true;; *) false;; esac'
out="$(call bypassPermissions PreToolUse t0 'Ship.' 'Ship today.')"
check "bypass mode refuses instead" 'case "$out" in *\"deny\"*) true;; *) false;; esac'
out="$(call default PreToolUse t0 'Ship.' 'Ship today.' codex)"
check "codex refuses instead" 'case "$out" in *\"deny\"*) true;; *) false;; esac'
mkdir -p "$CLAUDE_CONFIG_DIR"
echo '{"hooks":{"PermissionRequest":[{"matcher":"*","hooks":[]}]}}' > "$CLAUDE_CONFIG_DIR/settings.json"
out="$(call default PreToolUse t0 'Ship.' 'Ship today.')"
check "a hook that answers prompts makes lock refuse" 'case "$out" in *\"deny\"*) true;; *) false;; esac'
echo '{"hooks":{"PermissionRequest":[{"matcher":"AskUserQuestion","hooks":[]}]}}' > "$CLAUDE_CONFIG_DIR/settings.json"
call default PreToolUse t0 'Ship.' 'Ship today.' >/dev/null
out="$(call default PreToolUse t0 'Ship.' 'Ship today.')"
check "a hook for other tools does not" 'case "$out" in *\"ask\"*) true;; *) false;; esac'
rm "$CLAUDE_CONFIG_DIR/settings.json"
out="$(call default PostToolUse t1 'Ship.' 'Ship today.')"
check "a prompt answered no records nothing" '[ -z "$out" ] && [ -z "$(git status --porcelain .locks)" ]'
call default PreToolUse t3 'Ship.' 'Ship today.' >/dev/null
out="$(call default PreToolUse t3 'Ship.' 'Ship today.')"
edit 's/"Ship\."/"Ship today."/' src/app.ts
out="$(call default PostToolUse t3 'Ship.' 'Ship today.')"
check "a yes is recorded after the edit" 'case "$out" in *approved*) true;; *) false;; esac && "$LOCK" check >/dev/null'
expect "agent cannot add a hook that answers prompts" deny "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"ROOT/.claude/settings.json","content":"{\u0022a\u0022: \u0022lock hook lock hook lock hook lock hook\u0022\u002c \u0022PermissionRequest\u0022: []}"}}')"

# Cursor's own shapes after a command: an empty cwd, the workspace in workspace_roots,
# and two events, of which only postToolUse can tell the agent what lock put back.
cursor() { printf '{"conversation_id":"c1","cwd":"","workspace_roots":["%s"],"hook_event_name":"%s","tool_name":"Shell","tool_input":{"command":"%s","cwd":""},"command":"%s"}' "$ROOT" "$1" "$2" "$2" | "$LOCK" hook cursor; }
echo 'perl -pi -e '"'"'s/tagline = "[^"]*"/tagline = "Cursor."/; s/free = \d+/free = 7/'"'"' src/app.ts' > tools/cursor.sh
tagline_before="$(grep tagline src/app.ts)"
cursor beforeShellExecution "sh tools/cursor.sh" >/dev/null
sh tools/cursor.sh
after="$(cursor afterShellExecution "sh tools/cursor.sh")"
out="$(cursor postToolUse "sh tools/cursor.sh")"
check "cursor: locked text from a command is put back and reported" '[ "$after" = "{}" ] && [ "$(grep tagline src/app.ts)" = "$tagline_before" ] && grep -q "free = 7" src/app.ts && case "$out" in *additional_context*"put it back"*) true;; *) false;; esac'

# The lock hook entries may not change at all: Codex stops running a changed hook
# until a person reviews it again.
echo "perl -0pi -e 's/\"command\": \"/\"timeout\": 1, \"command\": \"/' .claude/settings.json" > tools/retime.sh
agent "sh tools/retime.sh"
check "a command that rewrites the lock hook entry is undone" '! grep -q "\"timeout\": 1" .claude/settings.json && case "$out" in *settings.json*) true;; *) false;; esac'
printf '[hooks.state.x]\ntrusted_hash = "sha256:1"\n' > "$CODEX_HOME/config.toml"
expect "agent cannot remove a Codex hook trust record" deny "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"'"$CDX"'/config.toml","content":"[hooks.state.x]\n"}}')"

# Audit reproductions
audit_base=$(git rev-parse HEAD)
audit_reset() {
  git reset -q --hard "$audit_base" 2>/dev/null || true
  git checkout -q -- src/app.ts .locks/files.json 2>/dev/null || true
  rm -f tools/forge-commit.sh tools/remove-lock-state.sh tools/fake-merge.sh tools/disarm-hook.sh tools/skip-hook.sh tools/rewrite-again.sh tools/latin-rewrite.sh
  rm -rf linkdir
  rm -f src/latin.ts .locks/approvals/audit-*.json
  rm -f "$(git rev-parse --git-dir)/MERGE_HEAD"
  rm -rf "$(git rev-parse --git-dir)/lock"
}

audit_reset
expect "audit: A1 case-folded .locks path is refused" deny "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"ROOT/.LOCKS/approvals/x.json","content":"{}"}}')"
expect "audit: A1 case-folded Claude settings path is refused" deny "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"ROOT/.CLAUDE/settings.json","content":"{}"}}')"

audit_reset
agent "ln -s .locks/approvals linkdir" || true
if [ -L linkdir ]; then
  expect "audit: A2 symlinked lock path is refused" deny "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"ROOT/linkdir/forged.json","content":"{}"}}')"
  rm -f linkdir
else
  rm -rf linkdir
  pass=$((pass+1)); echo "ok   audit: A2 symlinked lock path is refused (skipped on this OS)"
fi

audit_reset
cat > tools/forge-commit.sh <<'EOF'
#!/bin/sh
perl -pi -e 's/Pay for what you ship\./Forged./; s/Ship\./Forged./' src/app.ts
region() { sed -n '/LOCKED: tagline START/,/LOCKED: tagline END/p' | tr -d '\r'; }
from=$(git show HEAD:src/app.ts | region | sha256sum | cut -c1-64)
to=$(region < src/app.ts | sha256sum | cut -c1-64)
printf '{"version":1,"path":"src/app.ts","id":"tagline","before":"%s","after":"%s"}\n' "$from" "$to" > .locks/approvals/audit-forged.json
git add -A && git commit -qm forged
EOF
chmod +x tools/forge-commit.sh
head=$(git rev-parse HEAD)
agent "sh tools/forge-commit.sh" || true
check "audit: A3 forge and commit in one command is stopped" '[ "$(git rev-parse HEAD)" = "$head" ] || ! git show HEAD:src/app.ts 2>/dev/null | grep -q "tagline = \"Forged.\""; grep -q "tagline = \"Ship.\"" src/app.ts || grep -q "tagline = \"Pay for what you ship.\"" src/app.ts'

audit_reset
cat > tools/remove-lock-state.sh <<'EOF'
#!/bin/sh
perl -pi -e 's/Ship\./Deleted state./' src/app.ts
rm -rf "$(git rev-parse --git-dir)/lock"
EOF
agent "sh tools/remove-lock-state.sh" || true
check "audit: A4a deleting lock state is stopped" 'grep -q "tagline = \"Ship.\"" src/app.ts || grep -q "tagline = \"Pay for what you ship.\"" src/app.ts'

audit_reset
cat > tools/fake-merge.sh <<'EOF'
#!/bin/sh
perl -pi -e 's/Ship\./Fake merge./' src/app.ts
: > "$(git rev-parse --git-dir)/MERGE_HEAD"
EOF
agent "sh tools/fake-merge.sh" || true
check "audit: A4b faking a merge is stopped" 'grep -q "tagline = \"Ship.\"" src/app.ts || grep -q "tagline = \"Pay for what you ship.\"" src/app.ts'
rm -f "$(git rev-parse --git-dir)/MERGE_HEAD"

audit_reset
cat > tools/disarm-hook.sh <<'EOF'
#!/bin/sh
hook=$(git rev-parse --git-path hooks)/pre-commit
sed -i '2i exit 0' "$hook"
EOF
agent "sh tools/disarm-hook.sh" || true
check "audit: A4c disarming the commit hook is stopped" '! grep -qx "exit 0" "$(git rev-parse --git-path hooks)/pre-commit"'

audit_reset
cat > tools/skip-hook.sh <<'EOF'
#!/bin/sh
perl -pi -e 's/Ship\./Skipped hook./' src/app.ts
git commit -qam skip --no-verify
EOF
agent "sh tools/skip-hook.sh" || true
check "audit: A4d skipping the commit hook is stopped" '! git show HEAD:src/app.ts 2>/dev/null | grep -q "tagline = \"Skipped hook.\""'

audit_reset
base=$(git rev-parse HEAD)
edit 's/Ship\./Replay one./' src/app.ts
approve_region src/app.ts tagline
git add -A; git commit -qm audit-v1
edit 's/Replay one\./Ship./' src/app.ts
approve_region src/app.ts tagline
git add -A; git commit -qm audit-v0
git checkout -q -- src/app.ts
expect "audit: A5 approval replay is refused" deny "$(j '{"cwd":"ROOT","tool_name":"Edit","tool_input":{"file_path":"ROOT/src/app.ts","old_string":"Ship.","new_string":"Replay one."}}')"
git reset -q --hard "$base"

audit_reset
printf '/* LOCKED: bytes START */\nconst s = "caf\351";\n/* LOCKED: bytes END */\nconst marker = "\377";\n' > src/latin.ts
git add src/latin.ts; git commit -qm audit-latin
cat > tools/latin-rewrite.sh <<'EOF'
#!/bin/sh
LC_ALL=C perl -pi -e 's/\xe9/\xe8/g' src/latin.ts
EOF
agent "sh tools/latin-rewrite.sh" || true
check "audit: A6 non-UTF-8 locked bytes are restored" 'LC_ALL=C od -An -tx1 src/latin.ts | tr -d " \n" | grep -q e9 && LC_ALL=C od -An -tx1 src/latin.ts | tr -d " \n" | grep -q ff'
git rm -q -f src/latin.ts; git commit -qm audit-latin-cleanup

audit_reset
mkdir -p "$CLAUDE_CONFIG_DIR"
printf '{"disableAllHooks": false}\n' > "$CLAUDE_CONFIG_DIR/settings.json"
expect "audit: C3 flipping disableAllHooks is refused" deny "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"'"$CFG"'/settings.json","content":"{\u0022disableAllHooks\u0022: true}"}}')"
rm -f "$CLAUDE_CONFIG_DIR/settings.json"

audit_reset
echo 'perl -pi -e '\''s/Ship\./Repeated command./'\'' src/app.ts' > tools/rewrite-again.sh
hook PreToolUse "sh tools/rewrite-again.sh" >/dev/null
hook PreToolUse "sh tools/rewrite-again.sh" >/dev/null
sh tools/rewrite-again.sh
hook PostToolUse "sh tools/rewrite-again.sh" >/dev/null
check "audit: C1 identical commands keep the locked tagline" 'grep -q "tagline = \"Ship.\"" src/app.ts || grep -q "tagline = \"Pay for what you ship.\"" src/app.ts'

check "audit: C5 invalid baseline is rejected" '! "$LOCK" check --base no-such-ref >/dev/null 2>&1'

audit_reset
weird_hook() { printf '{"cwd":"%s","session_id":"audit-s1","hook_event_name":"%s","tool_name":"x.read","tool_use_id":"audit-x1","tool_input":{}}' "$ROOT" "$1" | "$LOCK" hook claude; }
weird_hook PreToolUse >/dev/null
edit 's/Ship\./Dotted tool./' src/app.ts
weird_hook PostToolUse >/dev/null
check "audit: S1 dotted tool names keep the locked tagline" 'grep -q "tagline = \"Ship.\"" src/app.ts || grep -q "tagline = \"Pay for what you ship.\"" src/app.ts'

audit_reset
sed -i 's/"catalog.json"/".\/catalog.json"/' .locks/files.json
git add .locks/files.json; git commit -qm audit-dot-path
expect "audit: S2 whole-file lock with ./ path is enforced" deny "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"ROOT/catalog.json","content":"{}"}}')"
git checkout -q HEAD~1 -- .locks/files.json
git add .locks/files.json; git commit -qm audit-dot-path-cleanup

# On a failure, show the agent settings, which several checks depend on.
[ "$fail" -eq 0 ] || { echo "--- .claude/settings.json"; cat .claude/settings.json; }
echo "passed $pass, failed $fail"
[ "$fail" -eq 0 ]
