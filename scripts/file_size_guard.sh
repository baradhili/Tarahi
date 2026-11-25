#!/usr/bin/env bash
# File-size guard (#364): every tracked .rs file over FILE_SIZE_CAP
# lines is reported, largest first, with its over-cap delta — the
# standing measuring stick for the decomposition effort (the app/
# split, pdf.rs #363, model.rs #362).
#
# WARNING-ONLY for now: the script exits 0 while violations remain, so
# CI stays green while the backlog drains. Set FILE_SIZE_ENFORCE=1 to
# make violations exit non-zero (the later flip to a hard gate; the
# ci.yml file-sizes job is where that goes).
#
# Excluded from the count: third_party/ (the vendored craft-ui fork,
# not ours to refactor) and generated files (first line declares it —
# e.g. dc-ui/src/l10n.rs is codegen output, not hand-refactorable).
#
# FILE_SIZE_REPORT=<path> additionally writes the report as the sticky
# PR-comment body (markdown table + the marker the ci.yml PR-comment
# step matches on, #366) — the comment format lives here so there is
# one place that owns it.
set -u

CAP="${FILE_SIZE_CAP:-1500}"
ENFORCE="${FILE_SIZE_ENFORCE:-0}"
in_actions=false
[ "${GITHUB_ACTIONS:-}" = "true" ] && in_actions=true

found="$(mktemp)"
trap 'rm -f "$found"' EXIT

while IFS= read -r f; do
    case "$f" in third_party/*) continue ;; esac
    if head -n 1 "$f" | grep -qi 'generated'; then continue; fi
    lines=$(wc -l < "$f")
    if [ "$lines" -gt "$CAP" ]; then
        printf '%d %s\n' "$lines" "$f" >> "$found"
    fi
done < <(git ls-files '*.rs')

count=$(wc -l < "$found")

# Sticky PR-comment body (#366): the ci.yml github-script step updates
# the bot's previous comment (matched by the marker) with this file.
# Written on BOTH paths — an all-clear run must still update the
# sticky comment so it drains to "no file exceeds the cap".
if [ -n "${FILE_SIZE_REPORT:-}" ]; then
    {
        echo '<!-- file-size-guard:sticky -->'
        echo
        echo '### File-size guard (#364)'
        echo
        if [ -s "$found" ]; then
            echo "**${count} file(s)** over the ${CAP}-line cap — warning-only:"
            echo
            echo '| Lines | File | Over |'
            echo '|---:|---|---:|'
            sort -rn "$found" | while IFS= read -r row; do
                lines=${row%% *}
                path=${row#* }
                printf '| %d | `%s` | +%d |\n' "$lines" "$path" "$((lines - CAP))"
            done
            echo
            echo '<sub>Warning-only until the decomposition backlog drains; `FILE_SIZE_ENFORCE=1` makes it a hard gate. Reproduce locally: `./scripts/file_size_guard.sh`.</sub>'
        else
            echo "No tracked \`.rs\` file exceeds the ${CAP}-line cap."
        fi
    } > "$FILE_SIZE_REPORT"
fi

if [ ! -s "$found" ]; then
    echo "file-size guard: no tracked .rs file exceeds ${CAP} lines."
    exit 0
fi

echo "file-size guard: ${count} file(s) over the ${CAP}-line cap (largest first):"
echo
while IFS= read -r row; do
    lines=${row%% *}
    path=${row#* }
    delta=$((lines - CAP))
    printf '%8d  %-52s +%d\n' "$lines" "$path" "$delta"
    if $in_actions; then
        kind=warning
        [ "$ENFORCE" = "1" ] && kind=error
        printf '::%s file=%s,title=File over the %s-line cap::%s lines (%d over) — refactor candidate (#364)\n' \
            "$kind" "$path" "$CAP" "$lines" "$delta"
    fi
done < <(sort -rn "$found")
echo
if [ "$ENFORCE" = "1" ]; then
    echo "file-size guard: ENFORCING — ${count} violation(s) over ${CAP} lines."
    exit 1
fi
echo "file-size guard: warning-only (#364) — ${count} file(s) need refactoring before this becomes a hard gate."
exit 0
