#!/usr/bin/env bash
set -euo pipefail
umask 077

destination="${1:?destination path required}"
repo_root="${2:-$(git rev-parse --show-toplevel)}"
chunk_dir="$repo_root/scripts/ops/new-product-fleets-20260804"
raw_expected_sha256=681149a614e9d4c6619c7c94d254b8ab374ae464d71aaf945fa45d892fc712bd
expected_sha256=abf745061eb32e01af46be6e1b5bc6f97abdb011aac87c8649e60ef06b23e274

test -d "$chunk_dir"
mapfile -t chunks < <(find "$chunk_dir" -maxdepth 1 -type f -name 'publisher.py.gz.b64.part-*' | sort)
test "${#chunks[@]}" = 4
mkdir -p "$(dirname "$destination")"
temporary="${destination}.tmp.$$"
cleanup() { rm -f "$temporary"; }
trap cleanup EXIT
cat "${chunks[@]}" | base64 --decode | gzip --decompress > "$temporary"
raw_observed="$(sha256sum "$temporary" | awk '{print $1}')"
test "$raw_observed" = "$raw_expected_sha256"

python3 - "$temporary" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
text = path.read_text(encoding="utf-8")
replacements = [
    ("cargo fmt --all -- --check", "cargo fmt --all", 2),
    ('Html(format!("<code>{}</code>", payload))', 'Html(format!("<code>{payload}</code>"))', 1),
    ('.route("/v1/events", post(publish_event))', '.route("/v1/realtime-events", post(publish_event))', 1),
    ('"POST /v1/events",', '"POST /v1/realtime-events",', 1),
    (
        'SEED_VALUES=", ".join(json.dumps(item) for item in product.samples),',
        'SEED_VALUES=", ".join(json.dumps(item, ensure_ascii=False) for item in product.samples),',
        1,
    ),
    ('assert_eq!(merged["local"], true);', 'assert!(merged["local"].as_bool().unwrap_or_default());', 1),
    ('assert_eq!(merged["remote"], true);', 'assert!(merged["remote"].as_bool().unwrap_or_default());', 1),
    (
        '''            if output == "pretty" {
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!("{}", serde_json::to_string(&payload)?);
            }
''',
        '''            let rendered = if output == "pretty" {
                serde_json::to_string_pretty(&payload)?
            } else {
                serde_json::to_string(&payload)?
            };
            println!("{rendered}");
''',
        1,
    ),
    (
        '''                        match update {
                            Ok(update) if sender.send(Message::Text(update)).await.is_err() => break,
                            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
''',
        '''                        match update {
                            Ok(update) => {
                                if sender.send(Message::Text(update)).await.is_err() {
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {}
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
''',
        1,
    ),
]
for needle, replacement, expected_count in replacements:
    observed_count = text.count(needle)
    if observed_count != expected_count:
        raise SystemExit(
            f"unexpected materializer replacement count for {needle!r}: "
            f"expected {expected_count}, observed {observed_count}"
        )
    text = text.replace(needle, replacement)
path.write_text(text, encoding="utf-8")
PY

observed="$(sha256sum "$temporary" | awk '{print $1}')"
test "$observed" = "$expected_sha256"
python3 -m py_compile "$temporary"
chmod 700 "$temporary"
mv "$temporary" "$destination"
trap - EXIT
printf 'materialized=%s raw_sha256=%s sha256=%s\n' "$destination" "$raw_observed" "$observed"
