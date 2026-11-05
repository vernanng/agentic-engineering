# Out-of-the-loop

Fully automatic. No prompts, no pauses — only guardrails:

- steps come only from the task template allowlist,
- a run is killed after 60 seconds,
- a step failure **halts the run**, writes an `alert` entry in the audit log, and **demotes the task type back to [near-the-loop](../near-the-loop/README.md)** automatically.

Use it only for task types that earned it (promotion from near after 5 consecutive successful runs). Humans supervise through the audit log and alerts, not per-step decisions.

Try it: set a task type to `out`, submit a run with `FAIL` in the input, and watch it fail, alert, and demote.
