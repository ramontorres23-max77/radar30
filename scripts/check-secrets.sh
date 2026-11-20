#!/bin/sh
# Refuses keys, tokens and personal home-folder paths in tracked or staged files.
# Staged files by default; --all checks every tracked file (or working tree if not a git repo).
# Enable once per clone: git config core.hooksPath .githooks
set -u

if [ "${1:-}" = "--all" ]; then
  if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    files=$(git ls-files)
  else
    files=$(find . -maxdepth 4 -type f \
      ! -path './target/*' ! -path './.git/*' | sed 's|^\./||')
  fi
  show() { cat -- "$1"; }
else
  files=$(git diff --cached --name-only --diff-filter=ACMR)
  show() { git show ":$1"; }
fi

patterns='BEGIN [A-Z ]*PRIVATE KEY|AKIA[0-9A-Z]{16}|gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,}|npm_[A-Za-z0-9]{36}|sk-(ant-|proj-)?[A-Za-z0-9_-]{24,}|xox[abpr]-[A-Za-z0-9-]{10,}|/Users/[A-Za-z0-9._-]+/|/home/[A-Za-z0-9._-]+/|[A-Za-z]:\\\\Users\\\\[A-Za-z0-9._-]+'
placeholders='/(Users|home)/(user|u|runner|me|you|name|example|someone|USER|<[^>]*>)/|[A-Za-z]:\\\\Users\\\\(user|runneradmin|USER)'

found=0
for f in $files; do
  case "$f" in
    scripts/check-secrets.sh) continue ;;
  esac
  hits=$(show "$f" 2>/dev/null | grep -nIE "$patterns" | grep -vE "$placeholders")
  if [ -n "$hits" ]; then
    printf '%s\n' "$hits" | sed "s|^|$f:|" | cut -c1-200
    found=1
  fi
done

if [ "$found" -ne 0 ]; then
  echo >&2 "check-secrets: the lines above look like a key, a token or a personal path."
  echo >&2 "Replace them with a placeholder (sk-test, /home/user/...), or commit with --no-verify if they are fakes."
  exit 1
fi
