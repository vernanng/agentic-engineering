# Loop App (learns)

A task earns autonomy: **in-the-loop >> near-the-loop >> out-of-the-loop**.

The runnable app now lives at the repo root — this folder is learns only (concepts, no duplicate code).

| Mode | Behaviour |
|---|---|
| in | Pauses before every step. Human must Approve, Approve with an edited output, or Reject. |
| near | Runs alone. Pauses only on low confidence (< 0.7), a risky step, or a failure (approve = skip step, reject = kill run). Pause / Resume / Kill from the run list. |
| out | No prompts. Guardrails only: 60s run timeout, step allowlist from the template. A failure halts the run, writes an `alert` audit entry, and demotes the task type back to near. |

Promotion is earned, not automatic: 3 consecutive successful runs in `in` (5 in `near`) make a task type eligible; a human clicks Promote. Manual mode buttons always override. Every run, step, decision, and mode change is in the audit log (SQLite).

## Modes

- [in-the-loop/](in-the-loop/README.md) — approve every step.
- [near-the-loop/](near-the-loop/README.md) — auto, pauses on exceptions, pause/resume/kill.
- [out-of-the-loop/](out-of-the-loop/README.md) — fully automatic, guardrails, alert + demotion on failure.

## Run the real app

```bash
python3 server.py          # from repo root: http://localhost:8000  (stdlib only)
python3 check.py           # from repo root: end-to-end check of all three modes + promotion
```

See the root [README.md](../../README.md) and [server.py](../../server.py). Edit `TEMPLATES` / `run_step()` there — do not recreate code in this learns folder.
