---
name: milestone-report
description: Write a Cuttlebone milestone or session report in docs/history/. Use when a milestone or a session's work is finished and needs its report.
---

# Writing a milestone report

1. Run `scripts/check.sh --milestone REV` (REV: the milestone's first
   commit) and make sure it passes: every mutation it selects must be
   caught. If it says the exhaustive tier is required (the mutation runner
   or shared test infrastructure changed), run `scripts/check.sh
   --exhaustive` too (AGENTS.md, "Checks").
2. Write the report as `docs/history/MILESTONE-<n>.md` (or a descriptive
   name such as `GUARDRAILS.md` for a session that isn't a milestone).
   Follow the shape of the earlier reports in docs/history/:
   - what was done, by task, and what tests each behaviour
   - measurements, with the machine they came from
   - every line added to `patches/upstream.patch`, and why (patches/README.md
     must agree)
   - the mutations, and that all were caught
   - decisions to revisit, and remaining unknowns
3. Link the report from README.md's list of reports.
4. Use repository-relative links and paths only.
5. Say in the report which tiers passed, on which commit, and how long
   each took.
6. Commit. The pre-commit hook runs `scripts/check.sh`.
