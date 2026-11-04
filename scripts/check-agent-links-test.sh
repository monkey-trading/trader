#!/usr/bin/env bash
# Checks check-agent-links.sh on a scratch repository: valid links, a missing file, a missing
# heading, titled links (quoted and parenthesised), reference definitions, angle-bracket links, a
# destination on the next line, a link that leaves the repository, a symlink that does,
# percent-encoded destinations, headings with closing markers, links into gitignored paths, skipped
# backtick and tilde fences, and wrong root symlinks. Prints one line per case and exits 1 if any
# case fails.
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
check() { # name expected-exit [output-line]
  local got=0
  "$repo/scripts/check-agent-links.sh" > "$tmp/out" 2>&1 || got=$?
  if [ "$got" != "$2" ]; then echo "FAIL $1: exit $got, want $2"; failed=1
  elif [ -n "${3:-}" ] && ! grep -qxF "$3" "$tmp/out"; then echo "FAIL $1: output lacks: $3"; failed=1
  else echo "ok   $1"; fi
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
fresh; touch "$repo/agents/rules/a b.md"; add '[x](a%20b.md)'; check valid-percent-encoded-link 0
fresh; add '[x](a%20b.md)'; check percent-encoded-link-missing 1 'agents/rules/a.md:4: a%20b.md does not resolve'
fresh; add '[x](a%2Emd#heading-one)'; check valid-percent-encoded-dot 0
fresh; touch "$tmp/outside.md"; add '[x](%2E%2E/%2E%2E/%2E%2E/outside.md)'; check encoded-leaves-repository 1 'agents/rules/a.md:4: %2E%2E/%2E%2E/%2E%2E/outside.md leaves the repository'
fresh; mkdir -p "$repo/agents/.processes"; touch "$repo/agents/.processes/n.md"; add '[x](../%2Eprocesses/n.md)'; check encoded-processes-link-reported 1 'agents/rules/a.md:4: ../%2Eprocesses/n.md is local only (gitignored)'
fresh; add '[x](a.md%00)'; check encoded-nul 1 'agents/rules/a.md:4: a.md%00 does not resolve'
fresh; printf '# C\n\n## Caf\xc3\xa9\n' > "$repo/agents/rules/c.md"; add '[x](c.md#caf%C3%A9)'; check valid-percent-encoded-fragment 0
fresh; add '[x](a.md#heading%20one)'; check percent-encoded-fragment-with-space 1 'agents/rules/a.md:4: a.md#heading%20one names no heading'
fresh; printf '## Usage ##\n' >> "$repo/agents/rules/a.md"; add '[x](#usage)'; check valid-closed-heading 0
fresh; printf '## Usage ##  \n' >> "$repo/agents/rules/a.md"; add '[x](#usage)'; check valid-closed-heading-trailing-space 0
fresh; printf '## Usage ##\n' >> "$repo/agents/rules/a.md"; add '[x](#usage-)'; check closed-heading-marker-not-in-anchor 1 'agents/rules/a.md:5: #usage- names no heading'
fresh; printf '## Usage ##\n' >> "$repo/agents/rules/a.md"; add '[x](#missing)'; check closed-heading-missing-fragment 1 'agents/rules/a.md:5: #missing names no heading'
fresh; printf '## Usage #\n## Usage ##\n' >> "$repo/agents/rules/a.md"; add '[x](#usage-1)'; check closed-headings-count-as-duplicates 0
fresh; printf '## C#\n' >> "$repo/agents/rules/a.md"; add '[x](#c)'; check hash-inside-heading-kept 0
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
fresh; rm "$repo/CLAUDE.md"; printf '[rule](agents/rules/a.md#heading-one)\n' > "$repo/CLAUDE.md"; check root-copy-not-symlink 1 'CLAUDE.md: not a symlink'
fresh; rm "$repo/AGENTS.md"; ln -s agents/rules/a.md "$repo/AGENTS.md"; check root-symlink-wrong-target 1 'AGENTS.md: does not point to agents/AGENTS.md'
fresh; mkdir -p "$repo/agents/.processes/fixes"; echo '[x](missing.md)' > "$repo/agents/.processes/x.md"; echo '[x](missing.md)' > "$repo/agents/.processes/fixes/y.md"; check processes-directory-skipped 0
exit "$failed"
