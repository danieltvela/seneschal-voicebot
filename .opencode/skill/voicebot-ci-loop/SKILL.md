---
name: voicebot-ci-loop
description: Iterate on Voicebot's Gitea CI until green. Run `make qa` locally, push the branch, watch the Gitea Actions run, fetch failed job logs, diagnose, fix, and re-push. Use this skill for any task that says "fix CI", "make CI green", "iterate on pipeline", "check Gitea", or "run tests via CI".
---

## Use This Skill

Put this line at the very top of any opencode prompt that needs to make CI green:

@voicebot-ci-loop

Then describe the work below it (e.g., "iterate CI on feature/issue-42 until green").

## Core Loop

1. Local gate via `make qa`
2. Push / re-push the branch
3. Watch the Gitea Actions run
4. Fetch logs of failed jobs
5. Diagnose and fix
6. Re-push and repeat
7. Escalate after 3 strikes on the same root cause

## Step 1 — Local Gate

Always run the local QA harness first. CI is for catching platform-specific issues, not for doing your work:

```bash
make qa
# or
bash scripts/qa.sh fast
```

The harness covers: `fmt`, `lint`, `test`, `test-ci`, `test-e2e`, `build`. If any of these fail locally, fix the code before pushing. Do not push known-broken code hoping CI will somehow pass.

Stages that are expected to `[SKIP]` in fast mode: `test-stt` (no Whisper model), `test-llm` (no LLM server), `audit` (cargo-audit not installed), `coverage` (cargo-llvm-cov not installed). These skips are normal and do not count as failures.

## Step 2 — Push / Re-push

```bash
git push origin feature/issue-N            # first push
git push --force-with-lease origin ...     # after fixup commits
```

Use `--force-with-lease`, never `--force` (per `AGENTS.md`).

The branch is on the project's Gitea remote (`tesla.local:3000/danielvela/voicebot`). Pushing to the branch with an open (or just-pushed) PR will trigger `.gitea/workflows/ci.yml` automatically — you do not normally need to dispatch the workflow manually.

## Step 3 — Watch the Gitea Run

Use the Gitea MCP tools. They are already available to opencode:

```text
gitea_actions_run_read  method=list_runs       owner=danielvela  repo=voicebot
gitea_actions_run_read  method=get_run         run_id=<N>        owner=danielvela  repo=voicebot
gitea_actions_run_read  method=list_run_jobs   run_id=<N>        owner=danielvela  repo=voicebot
```

### Polling pattern (do not tight-loop)

CI typically takes 4-12 minutes. Poll with a backoff:

```bash
# Pseudocode — adapt with the gitea_* tools
for i in 1 2 3 4 5 6 7 8 9 10 11 12; do
  status=$(gitea_actions_run_read method=get_run run_id=$N owner=danielvela repo=voicebot)
  case "$status" in
    success|failure|cancelled) break ;;
    *) sleep 30 ;;
  esac
done
```

If using `background_output` to wait on a long-running tool, use a `timeout` of at least 600000 ms (10 min) for the first wait.

## Step 4 — Fetch Logs of Failed Jobs

```text
gitea_actions_run_read  method=get_job_log_preview  job_id=<J>  tail_lines=200
```

Read the LAST 200 lines. The middle of a CI log is compilation noise; the error is at the end. If the error is in a step that runs on Linux but not on macOS, also check `build-linux` and `build-macos` jobs separately.

For very long logs, use `download_job_log` to write to a temp file under `/var/folders/.../opencode/`, then grep the file for `error\[:`, `error:`, `FAILED`, `panicked at`.

## Step 5 — Diagnose & Fix

### Common failure patterns

| Symptom in log | Likely cause | First action |
|---|---|---|
| `error[E0XXX]` in `src/...` | Code error / type mismatch | Read the file, run `lsp_diagnostics`, fix |
| `error: failed to run custom build command for \`ort\`` | Linux-only ONNX build issue | Check the job's OS image; verify Linux `apt` deps |
| `error: linking with \`cc\` failed` | Missing system lib on Linux | Check `.gitea/workflows/ci.yml` apt section |
| `clippy::...` violation | New lint regression | `cargo clippy --fix --allow-dirty` then review |
| `cargo fmt` check failed | Formatting drift | `cargo fmt` |
| `error: aborting due to N previous errors` (test) | A test failed | Re-run the specific test locally with `--nocapture` |
| `failed to fetch crate ...` | Network / registry issue in CI | Re-trigger; if persistent, check `Cargo.lock` for yanked crates |
| `thread 'main' panicked at 'WHISPER_MODEL not set'` | Expected skip mis-classified | Check `scripts/qa.sh` skip logic; tests should be `#[ignore]`d, not in `test-stt` stage |

### Platform-specific gotchas

- **Linux build of `kokorox 0.1.5`**: known broken against current `ort`. The harness and CI both avoid `--all-features`; do not "fix" this by adding `--all-features` to CI.
- **`parakeet` feature**: requires `PARAKEET_MODEL_DIR`. CI does not run parakeet tests.
- **`speaker` feature**: requires `models/speaker_embedding.onnx`. CI skips it.
- **macOS-only deps**: `objc2*` for `avspeech`. Linux CI does not compile this; if you touch `src/tts/avspeech.rs`, ensure `#[cfg(target_os = "macos")]` is intact.

## Step 6 — Re-push and Iterate

```bash
cargo fmt
cargo clippy --all-targets --no-deps --features tui,remote,control -- -D warnings
git add -A
git commit -m "fix: <one-line description>"
git push
```

Commit messages must be in English, short, and reference the issue number when applicable (e.g., `fix: address #42 — clippy drift in run_agent`).

## Step 7 — Strike Rule (Escalation)

- **3 consecutive CI failures with the same root cause** → STOP. Revert to the last known good commit. Post a comment on the issue describing what you tried. Ask the human before continuing.
- **A failure that looks flaky** (timeout, registry hiccup, transient OOM) → re-trigger the workflow with `gitea_actions_run_write method=rerun_run run_id=<N>`. If it persists on a second rerun, treat it as a real failure and diagnose.
- **A failure that introduces a NEW error** (different root cause than previous) → reset the strike counter. This counts as progress.

## Output Contract

End the loop with a compact summary:

```text
CI Status: GREEN | RED | ESCALATED
Iterations: N
Final commit: <sha>
PR: http://tesla.local:3000/danielvela/voicebot/pulls/<N>
Run URL: http://tesla.local:3000/danielvela/voicebot/actions/runs/<N>
Skipped stages (expected): test-stt, test-llm, audit, coverage
```

If escalating, also include:

```text
Last failing job: <name>
Last error: <one-line excerpt>
Tried: <bullet list of fix attempts>
```

## Anti-Patterns

- **DO NOT** poll `gitea_actions_run_read` in a tight synchronous loop. Use `background_output` with a timeout, or `sleep 30` between polls.
- **DO NOT** swallow the actual error from the log. Paste it verbatim in the issue comment.
- **DO NOT** use `git push --force`. Use `--force-with-lease`.
- **DO NOT** mix CI fixes with unrelated changes. One commit per fix; one PR per logical change.
- **DO NOT** add new tests, refactor code, or "improve" anything else while iterating on a CI fix. Stay surgical.
- **DO NOT** disable a failing test with `#[ignore]` to make CI green. Fix the test or the code.
- **DO NOT** use `as any`, `@ts-ignore`, or `#[allow(...)]` to silence the compiler. Solve the underlying problem.

## Reference — Local Test Commands

```bash
make qa                   # canonical pre-PR check (fast mode)
make qa-fast              # alias of make qa
make qa-full              # adds audit + coverage
make test                 # cargo test (default features)
make test-ci              # cargo test --features tui,remote,control
make test-e2e             # cargo test e2e -- --ignored
make test-stt             # requires WHISPER_MODEL
make test-llm             # requires LLM_URL reachable
make fmt                  # cargo fmt --all
make lint                 # cargo clippy --all-targets -- -D warnings
make build                # cargo build --release
make audit                # cargo audit (skipped if not installed)
make coverage             # cargo llvm-cov (skipped if not installed)
```

## Reference — Gitea MCP Tool Surface

| Tool | Method | Use |
|---|---|---|
| `gitea_actions_run_read` | `list_runs` | Find recent runs for a branch / PR |
| `gitea_actions_run_read` | `get_run` | Get run status (success / failure / running) |
| `gitea_actions_run_read` | `list_run_jobs` | List jobs in a run (check, test, build-macos, build-linux) |
| `gitea_actions_run_read` | `get_job_log_preview` | Tail the last N lines of a job log |
| `gitea_actions_run_read` | `download_job_log` | Save full log to a local file |
| `gitea_actions_run_write` | `dispatch_workflow` | Manually trigger a workflow (rare; pushes auto-trigger) |
| `gitea_actions_run_write` | `rerun_run` | Re-trigger a failed run after a fix |
| `gitea_actions_run_write` | `cancel_run` | Stop a runaway run |
| `gitea_pull_request_read` | `get` | Confirm PR head SHA matches pushed commit |
| `gitea_issue_write` | `add_comment` | Post the Output Contract summary to the issue |

All tools take `owner=danielvela repo=voicebot` plus the per-method arguments.
