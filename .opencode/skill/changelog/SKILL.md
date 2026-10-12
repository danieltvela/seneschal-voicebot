---
name: changelog
description: Generate a CHANGELOG.md entry from Gitea milestones. Use when the user types /changelog, asks to update the changelog, or needs to document release changes for a new tag.
---

## Use This Skill

Trigger with `/changelog` or when asked to generate release notes.

## Workflow

### 1. Find the last release tag

```text
gitea_list_tags  owner=danielvela  repo=seneschal-voicebot  perPage=10
```

The first tag in the list is the most recent release. Note the `name` field (e.g. `v0.1.0-alpha.3`).

### 2. Find the matching milestone

Milestones and tags share the same name. Find the milestone that matches the tag:

```text
gitea_milestone_read  method=list  owner=danielvela  repo=seneschal-voicebot
```

Look for the milestone whose `title` matches the tag name. Note the `id`.

### 3. Fetch closed issues for the milestone

List all issues filtered by milestone. Use the Gitea API to find issues with that milestone. If the milestone ID is known, list issues and filter by milestone field. Alternatively, list recent closed issues and check which ones belong to the milestone.

```text
gitea_list_issues  owner=danielvela  repo=seneschal-voicebot  state=closed  perPage=50
```

Cross-reference with the milestone. Issues that have the milestone set will appear in the milestone detail.

### 4. Fetch closed PRs for the milestone

```text
gitea_list_pull_requests  owner=danielvela  repo=seneschal-voicebot  state=closed  perPage=50
```

### 5. Get the release date

```text
gitea_get_tag  owner=danielvela  repo=seneschal-voicebot  tag_name=<TAG>
```

Use the tag's creation date, or fall back to the most recent commit date in the release.

### 6. Format the changelog entry

Generate a section in this format:

```markdown
## v0.1.0-alpha.4 (2026-07-08)

### Features
- **[#99](ISSUE_URL)**: App must start even if no audio device found
- **[#100](ISSUE_URL)**: Change the name of this project to Seneschal

### Bug Fixes
- **[#96](ISSUE_URL)**: Voicebot says it delegates task to Hermes, but ideas nothing
- **[#97](ISSUE_URL)**: Device monitor enabled is not working

### Other
- **[#94](ISSUE_URL)**: Investigate the use of system type messages
```

### 7. Prepend to CHANGELOG.md

Read `CHANGELOG.md`, keep the title line, insert the new section, then append the rest of the file. If the file doesn't exist, create it with a `# Changelog` title.

Use the `edit` tool to modify the file.

## Anti-Patterns

- **DO NOT** include open issues — only closed issues and merged PRs
- **DO NOT** include research-only issues that didn't result in code changes
- **DO NOT** overwrite existing changelog entries
- **DO NOT** include the current in-progress milestone — only completed releases