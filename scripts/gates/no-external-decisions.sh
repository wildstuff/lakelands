#!/usr/bin/env bash
# Gate 9 — no-external-decisions.
#
# Lakelands' rationale is its own, written in docs/decisions/, never a
# citation into a tree its readers cannot open. Red when a tracked file
# (code, docs, WIT, anything) or a commit message on the pull request
# carries one of the originating platform's reference forms:
#   - a decision-record citation;
#   - a planning-thread reference;
#   - a bare `#` followed by four or more digits. `lakelands#N` and full
#     URLs are allowed; so are HTML character references (`&#8212;`) and
#     colour literals in attribute, style or JSON-value position
#     (`fill="#000000"`, `color: #000000`, `"value": "#000000"`).
# The pull request's title (PR_TITLE, which a merge carries as its body)
# is read as a commit message is.
# Exempt is exactly GitHub's own merge subject, the first line of a
# commit message: "Merge pull request #N from <branch>", and nothing else.
# This script is the only file that may spell the patterns out.
. "$(dirname "$0")/lib.sh"

check() { # reads text on stdin, prints "<line>: <match>" per violation
    perl -ne '
        s{https?://\S+}{}g;                                  # full URLs
        s{&#[0-9]+;}{}g;                                     # character references
        s!(=\s*["\x27]|:\s*"?)#[0-9A-Fa-f]{3,8}(?=["\x27;}\s,)]|$)!$1!g;  # colours
        while (/(\bADR-[0-9]{4}\b|\bStrang\s+[0-9]+|(?<![A-Za-z0-9_])(?<!lakelands)#[0-9]{4,})/g) {
            print "$.: $1\n";
        }
    '
}

# Blanks the number of GitHub's merge subject on line 1, keeping the line.
merge_subject_out() {
    perl -pe 's{^(Merge pull request )#[0-9]+( from \S+)$}{$1$2} if $. == 1'
}

# A checker that does not compile would find nothing and pass.
check < /dev/null || die "the pattern checker does not run"

files=0
while IFS= read -r -d '' file; do
    grep -I -q '' "$file" 2>/dev/null || continue            # text files only
    files=$((files + 1))
    while IFS= read -r hit; do fail "$file:$hit"; done < <(check < "$file")
done < <(tracked_files)

range="$(gate_range)"
n=0
while IFS= read -r commit; do
    n=$((n + 1))
    while IFS= read -r hit; do
        fail "commit $(git log -1 --format='%h' "$commit") message line $hit"
    done < <(git log -1 --format='%B' "$commit" | merge_subject_out | check)
done < <(git rev-list "$range")

if [ -n "${PR_TITLE:-}" ]; then
    while IFS= read -r hit; do fail "pull request title line $hit"; done < <(printf '%s\n' "$PR_TITLE" | check)
fi

verdict "$files file(s), $n commit message(s)"
