#!/usr/bin/env bash
# Checks check-agent-links.sh on a scratch repository: valid links, a missing file, a missing
# heading, titled links (quoted and parenthesised), reference definitions, angle-bracket links, a
# destination on the next line, a link that leaves the repository, a symlink that does, links into
# gitignored paths, skipped backtick and tilde fences, and wrong root symlinks. Prints
# one line per case and exits 1 if any case fails.
set -euo pipefail
script="$(cd "$(dirname "$0")" && pwd)/check-agent-links.sh"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
repo="$tmp/repo"
failed=0

fresh() { # a minimal valid repository
  rm -rf "$repo"; mkdir -p "$repo/scripts" "$repo/agents/rules" "$repo/agents/skills/vendor/pack"
  cp "$script" "$repo/scripts/check-agent-links.sh"
  printf '# Index\n\n## Rules\n\n[rule](rules/a.md#heading-one)\n' > "$repo/agents/AGENTS.md"
  printf '# A\n\n## Heading one\n' > "$repo/agents/rules/a.md"
  ln -s agents/AGENTS.md "$repo/AGENTS.md"; ln -s agents/AGENTS.md "$repo/CLAUDE.md"
}
check() { # name expected-exit
  local got=0
  "$repo/scripts/check-agent-links.sh" > "$tmp/out" 2>&1 || got=$?
  if [ "$got" = "$2" ]; then echo "ok   $1"; else echo "FAIL $1: exit $got, want $2"; failed=1; fi
}
add() { printf '%s\n' "$1" >> "$repo/agents/rules/a.md"; }

fresh; check valid 0
fresh; add '[x](missing.md)'; check missing-file 1
fresh; add '[x](a.md#no-such-heading)'; check missing-heading 1
fresh; add '[x](a.md#heading-one)'; check valid-heading 0
fresh; add '[x](missing.md "Title")'; check titled-link 1
fresh; add '[x](missing.md (Title))'; check parenthesised-title 1
fresh; add '[x](a.md (Title))'; check valid-parenthesised-title 0
fresh; add '[x][r]'; add '[r]: missing.md'; check reference-definition 1
fresh; add '[x][r]'; add '[r]: a.md'; check valid-reference-definition 0
fresh; add '[x](<missing.md>)'; check angle-bracket-link 1
fresh; add '[x](<a.md>)'; check valid-angle-bracket-link 0
fresh; touch "$repo/agents/rules/a b.md"; add '[x](<a b.md>)'; check valid-angle-bracket-link-with-space 0
fresh; add '[x](<missing b.md>)'; check angle-bracket-link-with-space 1
fresh; add '[x]('; add '  missing.md)'; check destination-on-next-line 1
fresh; add '[x]('; add '  a.md)'; check valid-destination-on-next-line 0
fresh; add '[x](https://example.com/none)'; check external-link-ignored 0
fresh; add '`[x](missing.md)`'; check inline-code-ignored 0
fresh; printf '```\n[x](missing.md)\n```\n' >> "$repo/agents/rules/a.md"; check backtick-fence-ignored 0
fresh; printf '~~~md\n[x](missing.md)\n## Fake heading\n~~~\n' >> "$repo/agents/rules/a.md"; check tilde-fence-ignored 0
fresh; printf '~~~md\n## Fake heading\n~~~\n[x](a.md#fake-heading)\n' >> "$repo/agents/rules/a.md"; check fenced-heading-not-an-anchor 1
fresh; touch "$tmp/outside.md"; add '[x](../../../outside.md)'; check leaves-repository 1
fresh; touch "$tmp/outside.md"; ln -s "$tmp/outside.md" "$repo/agents/rules/out.md"; add '[x](out.md)'; check symlink-leaves-repository 1
fresh; touch "$repo/agents/skills/vendor/pull.sh"; add '[x](../skills/vendor/pull.sh)'; check tracked-vendor-file 0
fresh; touch "$repo/agents/skills/vendor/pack/SKILL.md"; add '[x](../skills/vendor/pack/SKILL.md)'; check vendor-pack-link-reported 1
fresh; mkdir -p "$repo/agents/.processes/fixes"; touch "$repo/agents/.processes/fixes/n.md"; add '[x](../.processes/fixes/n.md)'; check processes-link-reported 1
fresh; echo '[x](missing.md)' > "$repo/agents/skills/vendor/pack/README.md"; check vendor-pack-skipped 0
fresh; rm "$repo/CLAUDE.md"; cp "$repo/agents/AGENTS.md" "$repo/CLAUDE.md"; check root-copy-not-symlink 1
fresh; rm "$repo/AGENTS.md"; ln -s agents/rules/a.md "$repo/AGENTS.md"; check root-symlink-wrong-target 1
exit "$failed"
