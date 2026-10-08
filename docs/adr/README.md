# Architecture Decision Records

One file per decision, named `NNNN-short-title.md` and numbered in order. Numbers are never reused. A superseded ADR stays in place, with its status changed to "Superseded by NNNN".

Every dependency gets a line in an ADR, including its license (see MASTER_PROMPT §0 and §2).

## Index

None yet. The first ADRs come with the §13 plan.

## Template

```markdown
# NNNN. Title

- Status: Proposed | Accepted | Superseded by NNNN
- Date: YYYY-MM-DD

## Context

## Decision

## Alternatives considered

| Option | License | Why not |
|---|---|---|

## Consequences

## Evidence

Benchmarks (p50/p95, with machine spec) or spike results that support the decision.
```
