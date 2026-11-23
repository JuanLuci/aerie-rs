#!/bin/bash
# Fails when a change brings in what must never be in this repository:
# game files, captures, a server login, local paths, tokens, or the game's
# own package names. Usage: tools/ci/guard.sh <base ref>
set -u
base=${1:-origin/main}
bad=0
say() { echo "guard: $*"; bad=1; }

files=$(git diff --name-only --diff-filter=AMR "$base"...HEAD)
for f in $files; do
  case "$f" in
    *.zip|*.jar|*.class|*.bin|*.pak) say "$f: game files and captures never go in the repository" ;;
    *aerie-account.json) say "$f: a server login never goes in the repository" ;;
    aerie-world/*|gen/*) say "$f: a local world or generated table" ;;
    *.ps1) say "$f: local scripts hold local paths; keep them out" ;;
  esac
done

# Added lines only.
added=$(git diff -U0 "$base"...HEAD -- . ':(exclude)tools/ci/guard.sh' | grep '^+' | grep -v '^+++')
check() { # pattern, message
  if echo "$added" | grep -qiE "$1"; then
    say "$2"
    echo "$added" | grep -iE "$1" | head -3 | sed 's/^/    /' | cut -c1-160
  fi
}
check 'com\.hypixel|hypixel' "the game's own package names (code from elsewhere?)"
check 'eyJ[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{10,}' "something that looks like a token"
check '/home/[a-z]+/|C:\\\\Users\\\\|/Users/[A-Za-z]+/' "a local path"
check '(0x[0-9a-f]{2}, ?){16,}' "a long byte dump (captured data?)"

[ $bad = 0 ] && echo "guard: clean"
exit $bad
