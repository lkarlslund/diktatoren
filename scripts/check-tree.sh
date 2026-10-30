#!/usr/bin/env bash
# Fails when tracked (or staged) files include binaries, model weights, recordings, large
# files, or personal data such as home directory paths and e-mail addresses.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

mode="${1:-tracked}"
if [[ "$mode" == "--staged" ]]; then
    mapfile -t files < <(git diff --cached --name-only --diff-filter=ACMR)
else
    mapfile -t files < <(git ls-files)
fi

fail=0
complain() { echo "check-tree: $*" >&2; fail=1; }

allowed_binary='^(assets/icons/.*\.(png|ico|icns))$'
forbidden_ext='\.(gguf|safetensors|onnx|tflite|bin|wav|mp3|flac|ogg|opus|m4a|so|dylib|dll|exe|lib|a|o|pdb|zip|tar|gz|7z)$'
# Contact addresses that may legitimately appear in the tree.
allowed_email='^(noreply@github\.com|.*@users\.noreply\.github\.com|lars@karlslund\.dk)$'

for f in "${files[@]}"; do
    [[ -f "$f" ]] || continue
    [[ "$f" == third_party/* ]] && continue
    if [[ "$f" =~ $forbidden_ext ]]; then
        complain "forbidden file type: $f"
        continue
    fi
    size=$(wc -c <"$f")
    if (( size > 512000 )); then
        complain "file larger than 500 KB: $f"
    fi
    if [[ ! "$f" =~ $allowed_binary ]] && ! grep -Iq . "$f" 2>/dev/null && (( size > 0 )); then
        complain "binary file: $f"
        continue
    fi
    [[ "$f" == scripts/check-tree.sh ]] && continue
    if grep -nE '(/home/[a-z][a-z0-9_-]*/|/Users/[A-Za-z][A-Za-z0-9_-]*/|C:\\Users\\[A-Za-z])' "$f" >/dev/null 2>&1; then
        complain "personal path in $f: $(grep -nE '(/home/[a-z][a-z0-9_-]*/|/Users/[A-Za-z]|C:\\Users\\)' "$f" | head -1)"
    fi
    while read -r email; do
        [[ -z "$email" ]] && continue
        [[ "$email" =~ $allowed_email ]] || complain "e-mail address in $f: $email"
    done < <(grep -ohE '[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}' "$f" 2>/dev/null | sort -u)
done

exit "$fail"
