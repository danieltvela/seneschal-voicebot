# Git Workflow

## Commit Messages
- **English only**, short descriptive text identifying the change. No lengthy explanations.
- Small, focused commits are preferred.
- Example: `feat: add speaker verification module` or `fix: silence VAD false positives`

## Code Review
- **Local**: Review all code manually before committing (you).
- **CI/remote only**: When explicitly requested, allow the agent to commit, push, check CI logs, fix, and re-commit autonomously.
- Never auto-commit to main without explicit user instruction.

## Gitea Issues

Issues live on Gitea (`tesla.local:3000`). Use the Gitea MCP CLI for all issue operations (never `gh`, `tea` or raw `curl`).

### Documenting Work as Issue Comments

Every time an agent completes an analysis, plan, or finishes work on a Gitea issue, it **must** leave the results as a comment on that issue. This creates an auditable trail and lets the user review work without checking commits.

**When posting comments:**

| Trigger | What to include |
|---------|----------------|
| Starting analysis | Brief scope statement + what you'll investigate |
| Completing a plan | Numbered steps, affected files/modules, estimated complexity |
| Finishing implementation | Summary of changes, files touched, commands run, test results |
| Fixing a bug | Root cause, fix applied, verification steps |
| Research/spike | Findings, options considered, recommendation |

**Comment format:**

```markdown
## Analysis / Plan / Results

[Brief description of the work performed]

### Changes
- File/module affected: what changed and why

### Verification
- `cargo test` result
- `cargo clippy` clean
- Manual testing notes (if applicable)

### Related
- Commands run: `cargo run --features tui`
```

**Workflow for issue-driven work:**
1. Fetch issue details from Gitea with ots MCP.
2. Mark issue as in progress by adding label `ongoing`.
3. Post initial comment with scope/plan into the issue.
4. Execute the work.
5. Post final comment with results (mandatory) in the issue.

Labels exist but no issue templates — the agent handles formatting naturally.

## Versioning
- Semver with pre-release state: `v<major>.<minor>.<patch>-<state><number>`
- States: `alpha`, `beta`, `rc`
- Example: `v0.1.13-alpha01`, `v0.1.0-beta.1`
- Tag on main after validated merge.

## Git Worktrees (Isolated Binary Model)
To avoid context switching for the human and resource collisions, use an isolated binary model:
- **Human Zone:** `/Users/danielvela/projects/ai/seneschal` (Main stable context). Validates and merges work.
- **AI Zone:** `/Users/danielvela/projects/ai/seneschal-ai` (Autonomous cycle zone).
  - Agents MUST perform all work here.
  - When a task is completed, the worktree is cleared or moved to the next task.