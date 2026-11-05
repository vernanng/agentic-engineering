# Agentic Engineering

A task earns autonomy: **in-the-loop >> near-the-loop >> out-of-the-loop**.

| Mode | Behaviour |
|---|---|
| in | Pauses before every step. Human must Approve, Approve with an edited output, or Reject. |
| near | Runs alone. Pauses only on low confidence (< 0.7), a risky step, or a failure (approve = skip step, reject = kill run). Pause / Resume / Kill from the run list. |
| out | No prompts. Guardrails only: 60s run timeout, step allowlist from the template. A failure halts the run, writes an `alert` audit entry, and demotes the task type back to near. |

Promotion is earned, not automatic: 3 consecutive successful runs in `in` (5 in `near`) make a task type eligible; a human clicks Promote. Manual mode buttons always override. Every run, step, decision, and mode change is in the audit log (SQLite).

Build order is the same as promotion order: in-the-loop first, out-of-the-loop last. Do not start a new task type in `out`.

## Modes

- [in-the-loop/](in-the-loop/README.md) — approve every step.
- [near-the-loop/](near-the-loop/README.md) — auto, pauses on exceptions, pause/resume/kill.
- [out-of-the-loop/](out-of-the-loop/README.md) — fully automatic, guardrails, alert + demotion on failure.

## Run

```bash
python3 server.py          # http://localhost:8000  (stdlib only, nothing to install)
python3 check.py           # end-to-end check of all three modes + promotion
```

Docker:

```bash
docker build -t loop-app . && docker run --rm -p 8000:8000 -v loop-data:/data loop-app
```

Try it: submit a `summarize` run (in-the-loop, approve each step), switch `classify` to near and watch it pause on low confidence and the risky step, then put any task in out and add `FAIL` to the input to see the alert and demotion.

## Add a real task

Edit `TEMPLATES` in [server.py](server.py) (steps are `name, confidence, risky`) and replace `run_step()` with a real LLM/tool call keeping the same signature: `(step_name, text, confidence) -> (output, confidence)`, raising on failure. The loop, audit, and promotion logic do not change.

State is one SQLite file (`loop.db`, or `LOOP_DB`). Demo outputs are canned on purpose — the loop mechanics are the product here.

Auth: unset `LOOP_TOKEN` = localhost dev only. Before sharing, set `LOOP_TOKEN=<secret>` and send `X-Loop-Token: <secret>` (UI: `localStorage.setItem("loop_token", "<secret>")` then reload).

## Learns

[basics/](basics/README.md) — reference guides and demos (Python, TypeScript, Rust, Docker, REST, WebSockets, MD3). Frozen as learns; the real app lives in this root.

```bash
bash basics/check.sh    # check every basics demo
```
