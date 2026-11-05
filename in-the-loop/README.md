# In-the-loop

The starting mode. The runner pauses **before every step** and waits for a human.

- **Approve** — run the step as planned.
- **Approve with edit** — replace the step output with your own text; the step is marked done without running.
- **Reject** — the step is rejected and the run is killed.

Use it for any new task type. It is slow, but every decision is recorded in the audit log and builds the trust counters that earn promotion to [near-the-loop](../near-the-loop/README.md).

In the app: set the task type mode to `in`, submit a run, and use the Approve / Reject buttons on the selected run.
