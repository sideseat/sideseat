#!/usr/bin/env bash
# Capture OTLP fixtures for the message-parsing golden tests.
#
# Runs each sample against an in-process recorder (misc/record-otlp.py) so the exact bytes the
# framework emits land in server/tests/fixtures/messages/<suite>/<sample>/. Those fixtures are
# replayed by server/src/domain/traces/message_goldens_tests.rs, which checks message count,
# content, ordering and absence of duplicates across the span, trace and session views.
#
# Only Bedrock credentials are assumed; suites needing a first-party key are skipped.
#
# Usage:
#   misc/capture-message-fixtures.sh                 # every suite/sample below
#   misc/capture-message-fixtures.sh strands         # one suite
#   misc/capture-message-fixtures.sh strands tool_use  # one sample
#
# Re-running overwrites the fixture directory for the samples it covers and leaves the rest
# alone, so a single flaky sample can be re-captured without touching the others.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1

PORT="${RECORD_PORT:-5399}"
RECORD_ENDPOINT="http://127.0.0.1:${PORT}/otel/default"
FIXTURES="server/tests/fixtures/messages"

# suite|runner-template
# {S} is replaced by the sample name. Sample names are DISCOVERED from each CLI's --list
# rather than listed here: they differ per suite (bedrock has converse/invoke_model, not
# tool_use) and a hardcoded list silently skips or fails whole suites when it drifts.
SUITES=(
  "strands|uv run --directory misc/samples/python/strands strands {S}"
  "langgraph|uv run --directory misc/samples/python/langgraph langgraph {S}"
  "crewai|uv run --directory misc/samples/python/crewai crewai {S}"
  "adk|uv run --directory misc/samples/python/adk telemetry-adk {S}"
  "bedrock|uv run --directory misc/samples/python/bedrock bedrock {S}"
  "openai|uv run --directory misc/samples/python/openai openai-provider {S}"
  "openai-agents|uv run --directory misc/samples/python/openai-agents openai-agents {S}"
  "anthropic|uv run --directory misc/samples/python/anthropic anthropic-provider {S}"
  "agent-framework|uv run --directory misc/samples/python/agent-framework agent-framework {S}"
  "claude-agent-sdk|uv run --directory misc/samples/python/claude-agent-sdk claude-agent-sdk {S}"
  # Listed so the inventory is complete and its absence is visible rather than silent. Skipped
  # unless a first-party key is present: autogen's runner has no Bedrock path, so it cannot run
  # on the AWS credentials every other suite uses.
  "autogen|uv run --directory misc/samples/python/autogen autogen {S}"
  "vercel-ai-js|cd misc/samples/js && npm run vercel-ai -- {S}"
  "strands-js|cd misc/samples/js && npm run strands -- {S}"
  "claude-agent-sdk-js|cd misc/samples/js && npm run claude-agent-sdk -- {S}"
)

# Samples that must not be captured, with the reason.
# strands_ws blocks until SIGINT (it hosts agents over the WS runtime channel), so it would
# hang the capture rather than finish.
SKIP_SAMPLES="strands_ws all"

# Ask the CLI which samples it has. `--list` prints them indented under a header, followed by
# a "Model Aliases:" section, so take indented single-word lines before that section.
discover_samples() {
  local runner="$1"
  local cmd="${runner//\{S\}/--list}"
  bash -c "$cmd" 2>/dev/null | awk '
    /Model Aliases:/ { exit }
    /^  [a-z][a-z0-9_-]*$/ { gsub(/^  /, ""); print }
  '
}

want_suite="${1:-}"
want_sample="${2:-}"

recorder_pid=""
ENV_FILE="misc/.env"
ENV_BACKUP="$(mktemp -t sideseat-env-backup)"
env_redirected=0

# The sample CLIs call load_dotenv(..., override=True), so misc/.env wins over anything
# exported here. Point its endpoint at the recorder for the duration of the capture and put
# the original back on every exit path, including Ctrl-C.
redirect_env() {
  [[ -f "$ENV_FILE" ]] || return 0
  cp "$ENV_FILE" "$ENV_BACKUP"
  env_redirected=1
  python3 - "$ENV_FILE" "$RECORD_ENDPOINT" <<'PYEOF'
import re, sys
path, endpoint = sys.argv[1], sys.argv[2]
text = open(path).read()
if re.search(r'^OTEL_EXPORTER_OTLP_ENDPOINT=.*$', text, re.M):
    text = re.sub(r'^OTEL_EXPORTER_OTLP_ENDPOINT=.*$',
                  f'OTEL_EXPORTER_OTLP_ENDPOINT={endpoint}', text, flags=re.M)
else:
    text = text.rstrip('\n') + f'\nOTEL_EXPORTER_OTLP_ENDPOINT={endpoint}\n'
open(path, 'w').write(text)
PYEOF
}

restore_env() {
  if [[ "$env_redirected" == "1" && -f "$ENV_BACKUP" ]]; then
    cp "$ENV_BACKUP" "$ENV_FILE"
    env_redirected=0
    echo "[capture] restored $ENV_FILE"
  fi
  rm -f "$ENV_BACKUP"
}

cleanup() {
  if [[ -n "$recorder_pid" ]] && kill -0 "$recorder_pid" 2>/dev/null; then
    kill "$recorder_pid" 2>/dev/null
    wait "$recorder_pid" 2>/dev/null
  fi
}

cleanup_all() {
  cleanup
  restore_env
}
trap cleanup_all EXIT INT TERM

# A stale recorder from an interrupted run keeps the port bound. The readiness probe below
# would then connect to *it* and every capture would land under its old label, silently, so
# clear our own strays and refuse to continue if anything else still holds the port.
pkill -f "record-otlp.py --label" 2>/dev/null && sleep 1
if lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
  echo "[capture] port $PORT is already in use by another process; set RECORD_PORT to a free port" >&2
  exit 1
fi

redirect_env

total=0
ok=0
failed=()

for entry in "${SUITES[@]}"; do
  IFS='|' read -r suite runner <<<"$entry"
  [[ -n "$want_suite" && "$suite" != "$want_suite" ]] && continue

  # Suites with no Bedrock path need a first-party key; skip rather than report a failure that
  # is really a missing credential.
  if [[ "$suite" == "autogen" && -z "${OPENAI_API_KEY:-}${ANTHROPIC_API_KEY:-}" ]]; then
    echo "[capture] $suite: skipped - needs OPENAI_API_KEY or ANTHROPIC_API_KEY (no Bedrock path)"
    continue
  fi

  samples="$(discover_samples "$runner")"
  if [[ -z "$samples" ]]; then
    echo "[capture] $suite: could not list samples (is the suite installed?)"
    failed+=("$suite/<list>")
    continue
  fi

  for sample in $samples; do
    [[ -n "$want_sample" && "$sample" != "$want_sample" ]] && continue
    if [[ " $SKIP_SAMPLES " == *" $sample "* ]]; then
      echo "[capture] $suite/$sample: skipped by SKIP_SAMPLES"
      continue
    fi
    total=$((total + 1))
    label="${suite}/${sample}"
    echo ""
    echo "=============================================================="
    echo "[capture] $label"
    echo "=============================================================="

    rm -rf "${FIXTURES:?}/${label}"

    recorder_log="/tmp/recorder-$$-${suite}-${sample}.log"
    python3 misc/record-otlp.py --label "$label" --port "$PORT" >"$recorder_log" 2>&1 &
    recorder_pid=$!
    # Wait for OUR recorder to report it is listening. An open port is not enough: a stray
    # listener would satisfy a bare connect() while writing fixtures somewhere else.
    ready=0
    for _ in $(seq 1 100); do
      if grep -q "listening on" "$recorder_log" 2>/dev/null; then ready=1; break; fi
      if ! kill -0 "$recorder_pid" 2>/dev/null; then break; fi
      sleep 0.1
    done
    if [[ "$ready" != "1" ]]; then
      echo "[capture] $label: recorder failed to start"
      tail -5 "$recorder_log"
      failed+=("$label")
      cleanup
      recorder_pid=""
      continue
    fi

    cmd="${runner//\{S\}/$sample}"
    sample_log="/tmp/sample-$$-${suite}-${sample}.log"
    if OTEL_EXPORTER_OTLP_ENDPOINT="$RECORD_ENDPOINT" \
       SIDESEAT_ENDPOINT="http://127.0.0.1:${PORT}" \
       timeout 600 bash -c "$cmd" >"$sample_log" 2>&1; then
      sample_status="ran"
    else
      sample_status="FAILED(exit=$?)"
    fi

    # Give the exporter a moment to flush its final batch before the recorder dies.
    sleep 3
    cleanup
    recorder_pid=""

    captured=$(find "${FIXTURES}/${label}" -type f 2>/dev/null | wc -l | tr -d ' ')

    # Refuse to keep a fixture carrying credential material. Some frameworks serialise their
    # whole model config into a span attribute - CrewAI's agent_core dumped a live
    # aws_secret_access_key and aws_session_token - and a captured payload goes straight into
    # git history, where a secret cannot be taken back.
    #
    # Two checks, because each catches what the other misses. `gitleaks` knows hundreds of
    # credential shapes and is the primary gate; the pattern list below is the fallback for a
    # machine without it, and covers the shapes that have actually turned up here.
    #
    # The narrow version of this guard matched exactly two snake_case AWS field names, and that is
    # how `aws.auth.account.access_key` reached three committed fixtures with four STS key ids in
    # it: a dotted OTel attribute name matched nothing. Field names are not the thing to enumerate -
    # the *values* are.
    secret_hit=""
    if [[ -d "${FIXTURES}/${label}" ]]; then
      if command -v gitleaks >/dev/null 2>&1; then
        gitleaks detect --no-git --no-banner --redact \
          --source "${FIXTURES}/${label}" >/dev/null 2>&1 || secret_hit="gitleaks"
      else
        # Loud, not silent: a guard whose strength depends on whether a tool happens to be
        # installed, without saying so, is a guard nobody can rely on.
        echo "[capture] $label: WARNING gitleaks not installed - only the fallback patterns ran"
      fi

      # Value shapes, not field names. AWS long-term and STS key ids; secret keys and session
      # tokens in either snake_case or camelCase; bearer tokens; generic api keys; SigV4
      # signatures in a presigned URL.
      if [[ -z "$secret_hit" ]] && grep -rlqE \
          "(AKIA|ASIA)[A-Z0-9]{16}|(aws_?secret_?access_?key|secretAccessKey)['\"]?[[:space:]]*[:=][[:space:]]*['\"][^'\"]{8,}|(aws_?session_?token|sessionToken)['\"]?[[:space:]]*[:=][[:space:]]*['\"][^'\"]{20,}|[Bb]earer[[:space:]]+[A-Za-z0-9._~+/-]{20,}|(api[_-]?key|apiKey)['\"]?[[:space:]]*[:=][[:space:]]*['\"][^'\"]{16,}|X-Amz-Signature=[a-f0-9]{32,}" \
          "${FIXTURES}/${label}" 2>/dev/null; then
        secret_hit="pattern"
      fi
    fi
    if [[ -n "$secret_hit" ]]; then
      echo "[capture] $label: SECRET MATERIAL in payload ($secret_hit) - DISCARDING fixture"
      rm -rf "${FIXTURES:?}/${label}"
      failed+=("$label(secrets)")
      continue
    fi

    # Surface payloads big enough to be worth a decision before they reach git history: the
    # JS image suites inline base64 images and produce 7-15MB per request.
    while IFS= read -r big; do
      [[ -n "$big" ]] && echo "[capture] $label: LARGE payload $(du -h "$big" | cut -f1) $big - consider .gitignore"
    done < <(find "${FIXTURES}/${label}" -type f -size +1M 2>/dev/null)
    # The `error` samples exist to produce error telemetry and exit non-zero; that is the
    # behaviour under test, not a truncated run. Treating their exit code as failure discarded
    # 28 suites' worth of error-path coverage.
    expected_failure=0
    [[ "$sample" == "error" ]] && expected_failure=1

    if [[ "$captured" -gt 0 && ("$sample_status" == "ran" || "$expected_failure" == "1") ]]; then
      if [[ "$expected_failure" == "1" && "$sample_status" != "ran" ]]; then
        echo "[capture] $label: $sample_status (expected for an error sample), $captured request(s) recorded"
      else
        echo "[capture] $label: ran, $captured request(s) recorded"
      fi
      ok=$((ok + 1))
    elif [[ "$captured" -gt 0 ]]; then
      # Partial capture: the sample died part-way, so the fixture is a truncated conversation
      # that would be recorded as if it were the whole thing. Discard it rather than bless it.
      echo "[capture] $label: $sample_status after $captured request(s) - DISCARDING partial fixture"
      echo "--- last 15 lines of sample output ---"
      tail -15 "$sample_log"
      rm -rf "${FIXTURES:?}/${label}"
      failed+=("$label(partial)")
    else
      echo "[capture] $label: $sample_status, NO fixtures captured"
      echo "--- last 15 lines of sample output ---"
      tail -15 "$sample_log"
      failed+=("$label")
    fi
  done
done

echo ""
echo "=============================================================="
echo "[capture] $ok/$total samples produced fixtures"
if ((${#failed[@]})); then
  echo "[capture] no fixtures for: ${failed[*]}"
fi
echo "[capture] fixtures under $FIXTURES"
echo "[capture] next: UPDATE_GOLDENS=1 cargo test -p sideseat-server message_goldens   # record"
echo "[capture]       misc/review-message-goldens.py                                    # read the result"
echo "[capture]       cargo test -p sideseat-server message_goldens                     # then it gates"

# Non-zero when anything failed. Reporting failures and exiting 0 meant a CI step or a caller
# chaining with && treated a partial capture as a full one.
if ((${#failed[@]})); then
  exit 1
fi
