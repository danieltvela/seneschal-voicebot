#!/usr/bin/env bash
cd /home/danielvela/projects/ai/voicebot
export QA_NO_COLOR=1
run() { # stage, timeout-seconds
  local s="$1" t="$2"
  echo "@@@@@ STAGE $s @@@@@"
  timeout "$t" bash scripts/qa.sh "$s" > ".qwen/tmp/qa/$s.log" 2>&1
  echo "@@@@@ EXIT($s)=$? @@@@@"
}
run test 1500
run test-llm 1500
run test-stt 2400
run test-ci 1800
run test-e2e 1800
run lint 1800
run build 1800
echo "@@@@@ ALL DONE @@@@@"
