#!/usr/bin/env bash
set -euo pipefail

# Use a directory with text files to stress-test; no default to avoid leaking paths.
TEMPLATE_PROJECTS_PATH="${TEMPLATE_PROJECTS_PATH:?TEMPLATE_PROJECTS_PATH must be set to a directory with files for stress test}"
SLEEP_SECONDS="${SLEEP_SECONDS:-0.2}"
INDEX_READY_TIMEOUT_SECONDS="${INDEX_READY_TIMEOUT_SECONDS:-10}"

if [ ! -d "$TEMPLATE_PROJECTS_PATH" ]; then
  echo "Template path not found: $TEMPLATE_PROJECTS_PATH" >&2
  exit 1
fi

TMP_DIR="$(mktemp -d)"
cleanup() {
  if [ -n "${TMP_DIR:-}" ] && [ -d "$TMP_DIR" ]; then
    rm -rf "$TMP_DIR"
  fi
}
trap cleanup EXIT

echo "Temp dir: $TMP_DIR"
ftm checkout "$TMP_DIR"

cp -a "$TEMPLATE_PROJECTS_PATH" "$TMP_DIR/"

INDEX_JSON="$TMP_DIR/.ftm/index.json"
wait_for_index() {
  local deadline=$((SECONDS + INDEX_READY_TIMEOUT_SECONDS))
  while [ ! -f "$INDEX_JSON" ]; do
    if [ "$SECONDS" -ge "$deadline" ]; then
      echo "index.json not found within timeout: $INDEX_JSON" >&2
      exit 1
    fi
    sleep 0.05
  done
}

history_count() {
  wait_for_index
  python3 - "$INDEX_JSON" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as f:
    data = json.load(f)

history = data.get("history", [])
print(len(history))
PY
}

sleep "$SLEEP_SECONDS"
k="$(history_count)"

if [ "$k" -le 0 ]; then
  echo "Invalid history count k=$k" >&2
  exit 1
fi

echo "Initial history count k=$k"

project_name="$(basename "$TEMPLATE_PROJECTS_PATH")"
multiplier=2
iteration=1

while true; do
  echo "Iteration $iteration: delete"
  rm -rf "$TMP_DIR/$project_name"
  sleep "$SLEEP_SECONDS"
  count="$(history_count)"
  expected=$((k * multiplier))
  if [ "$count" -ne "$expected" ]; then
    echo "Mismatch after delete: expected $expected, got $count"
    exit 1
  fi
  echo "OK after delete: $count"
  multiplier=$((multiplier + 1))

  echo "Iteration $iteration: copy"
  cp -a "$TEMPLATE_PROJECTS_PATH" "$TMP_DIR/"
  sleep "$SLEEP_SECONDS"
  count="$(history_count)"
  expected=$((k * multiplier))
  if [ "$count" -ne "$expected" ]; then
    echo "Mismatch after copy: expected $expected, got $count"
    exit 1
  fi
  echo "OK after copy: $count"
  multiplier=$((multiplier + 1))
  iteration=$((iteration + 1))
done
