#!/usr/bin/env bash
# Scenario checks: runs the lock binary against hook payloads and git states in a throwaway repository.
set -u
LOCK="${LOCK:?set LOCK to the lock binary}"
export PATH="$(cd "$(dirname "$LOCK")" && pwd):$PATH"
WORK="$(mktemp -d)"
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
  out="$(printf '%s' "$3" | "$LOCK" hook claude)"; code=$?
  if [ -n "$out" ]; then got=deny; else got=allow; fi
  [ $code -eq 2 ] && got="error"
  if [ "$got" = "$2" ]; then pass=$((pass+1)); echo "ok   $1"; else fail=$((fail+1)); echo "FAIL $1 (got $got) $out"; fi
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
expect "shell sed -i on locked file" deny "$(j '{"cwd":"ROOT","tool_name":"Bash","tool_input":{"command":"sed -i s/Pay/Ship/ src/app.ts"}}')"
expect "shell redirect into changelog" deny "$(j '{"cwd":"ROOT","tool_name":"Bash","tool_input":{"command":"echo x > CHANGELOG.md"}}')"
expect "shell rm locked dir" deny "$(j '{"cwd":"ROOT","tool_name":"Bash","tool_input":{"command":"rm -rf src"}}')"
expect "shell approve" deny "$(j '{"cwd":"ROOT","tool_name":"Bash","tool_input":{"command":"lock approve tagline"}}')"
expect "powershell set-content" deny "$(j '{"cwd":"ROOT","tool_name":"PowerShell","tool_input":{"command":"Set-Content -Path CHANGELOG.md -Value x"}}')"
expect "shell write to .locks" deny "$(j '{"cwd":"ROOT","tool_name":"Bash","tool_input":{"command":"cp /tmp/a.json .locks/approvals/a.json"}}')"
expect "shell read locked file" allow "$(j '{"cwd":"ROOT","tool_name":"Bash","tool_input":{"command":"cat src/app.ts | head -3 2>&1"}}')"
expect "shell write elsewhere" allow "$(j '{"cwd":"ROOT","tool_name":"Bash","tool_input":{"command":"echo x > notes.txt"}}')"
expect "remove lock hook from settings" deny "$(j '{"cwd":"ROOT","tool_name":"Write","tool_input":{"file_path":"ROOT/.claude/settings.json","content":"{\u0022disableAllHooks\u0022: true}"}}')"
expect "patch on locked file" deny "$(j '{"cwd":"ROOT","tool_name":"ApplyPatch","tool_input":{"patch":"*** Begin Patch\n*** Update File: src/app.ts\n@@\n-a\n+b\n*** End Patch"}}')"

# Codex shapes: patches are replayed, so a patch outside a region passes.
expect "codex patch outside region" allow "$(j '{"cwd":"ROOT","tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Update File: src/app.ts\n@@\n-export const free = 1;\n+export const free = 9;\n*** End Patch"}}')"
expect "codex patch inside region" deny "$(j '{"cwd":"ROOT","tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Update File: src/app.ts\n@@\n /* LOCKED: tagline START */\n-export const tagline = \"Pay for what you ship.\";\n+export const tagline = \"Ship.\";\n*** End Patch"}}')"
expect "codex patch deletes locked file" deny "$(j '{"cwd":"ROOT","tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Delete File: CHANGELOG.md\n*** End Patch"}}')"
expect "codex shell apply_patch" deny "$(j '{"cwd":"ROOT","tool_name":"shell","tool_input":{"command":["bash","-lc","apply_patch <<EOF\n*** Begin Patch\n*** Update File: CHANGELOG.md\n@@\n-Initial release.\n+Changed.\n*** End Patch\nEOF"]}}')"
expect "codex shell array sed" deny "$(j '{"cwd":"ROOT","tool_name":"exec_command","tool_input":{"cmd":["sed","-i","s/Pay/Ship/","src/app.ts"]}}')"

# Cursor shapes
cur="$(printf '%s' "$(j '{"cwd":"ROOT","command":"echo x > CHANGELOG.md"}')" | "$LOCK" hook cursor)"
case "$cur" in *'"permission":"deny"'*) pass=$((pass+1)); echo "ok   cursor shell deny";; *) fail=$((fail+1)); echo "FAIL cursor shell deny $cur";; esac
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
git commit -qm sneaky 2>/dev/null && { fail=$((fail+1)); echo "FAIL pre-commit blocks"; } || { pass=$((pass+1)); echo "ok   pre-commit blocks"; }
git commit -qm sneaky --no-verify
"$LOCK" check --base HEAD~1 2>/dev/null && { fail=$((fail+1)); echo "FAIL ci base catches --no-verify"; } || { pass=$((pass+1)); echo "ok   ci base catches --no-verify"; }
echo y | "$LOCK" approve 2>/dev/null && { fail=$((fail+1)); echo "FAIL approve refuses piped input"; } || { pass=$((pass+1)); echo "ok   approve refuses piped input"; }

echo "passed $pass, failed $fail"
[ "$fail" -eq 0 ]
