# Near-the-loop

The supervised mode. The runner works alone and only stops for a human when something needs judgement:

- step confidence < 0.7 (**low confidence**),
- step is flagged **risky**,
- a step **fails** — Approve skips the step, Reject kills the run.

From the run list a human can also **Pause**, **Resume**, or **Kill** any run at any time.

Use it once a task type has proved itself in [in-the-loop](../in-the-loop/README.md) (3 consecutive successful runs make it eligible for promotion). After 5 more clean runs it can earn [out-of-the-loop](../out-of-the-loop/README.md).
