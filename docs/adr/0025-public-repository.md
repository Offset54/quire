# 0025. Public repository

- Status: Accepted (user decision, 2026-10-13)
- Date: 2026-10-13

## Context

The repo was private (decided on plan approval, 2026-10-08), so GitHub Actions minutes were billed against the free plan's 2,000 per month, with Windows at ×2 and macOS at ×10. From 2026-10-01 to 2026-10-12 there were 47 workflow runs, about 1,900 billable minutes:

| Job | Minutes | Billable |
|---|---|---|
| CI › macOS build and test | 81 | 810 |
| CI › Windows (all gates) | 244 | 488 |
| CI › Windows UI flows (WebDriver) | 119 | 238 |
| CI › Linux build and test | 163 | 163 |
| Fuzz (4 targets, nightly) | 149 | 149 |
| CI › licenses and advisories | 50 | 50 |

Measured from job start and end times (`gh run view --json jobs`), each job rounded up to whole minutes. At that rate the quota runs out in under two weeks.

## Decision

Make `Offset54/quire` public. Standard GitHub-hosted runners are free for public repos, so the CI matrix and nightly fuzzing stay as they are.

Before the switch, the full history was scanned: no keys, tokens, private keys or credential files; no blob over 1 MB. All commits carry the author's personal email; the user accepted that. New commits in this checkout use the GitHub noreply address.

## Alternatives considered

| Option | Why not |
|---|---|
| Stay private, trim CI (macOS/Linux on PRs or nightly only, skip docs-only changes, shorter fuzzing) | Roughly a quarter of current usage, but less cross-platform coverage on every push |
| No Actions for a month; Windows checks locally, macOS/Linux paused | Loses the clean-environment check; `#[cfg(windows)]`-gated code can break the other platforms unseen |
| Rewrite history to the noreply email before going public | Changes every commit hash and breaks concurrent sessions; the user accepted the exposure instead |

## Consequences

- Code, docs (including `docs/MASTER_PROMPT.md`) and full history are public. Forks or clones made from now on can't be withdrawn by going private again.
- There is no `LICENSE` file, so the code is all rights reserved. Choosing a license is still open (RISKS watch list).
- Concurrency limits still apply (about 20 jobs, 5 of them macOS), and the Actions cache is still 10 GB per repo.
- Pull requests from forks run CI. The workflows use `pull_request` (not `pull_request_target`) with `contents: read` and no secrets; keep it that way.
- The codename `quire` is public until the real name is chosen (before M7).
