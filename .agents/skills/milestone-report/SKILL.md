---
name: milestone-report
description: Write a Cuttlebone milestone or session report in docs/history/. Use when a milestone or a session's work is finished and needs its report.
---

# Writing a milestone report

1. Run `scripts/check.sh --full` and make sure it passes. It adds the
   mutation runner to the usual checks, and every mutation must be caught.
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
5. Say in the report that `scripts/check.sh --full` passed, and how long it
   took.
6. Commit. The pre-commit hook runs `scripts/check.sh`.
