# Agentic Engineering

Real app under active development — **local first**. No cloud deploy; everything runs on this machine via Docker Compose at `http://localhost:8000`.

A task earns autonomy: **in-the-loop >> near-the-loop >> out-of-the-loop**.

| Mode | Behaviour |
|---|---|
| `in` | Pauses before every step. Human must Approve, Approve with an edited output, or Reject (reject kills the run). |
| `near` | Runs alone. Pauses only on low confidence (< 0.7), a risky step, or a failure (approve = skip step, reject = kill run). Pause / Resume / Kill from the run list. |
| `out` | No prompts. Guardrails only: 60s run timeout, steps from the template allowlist. A failure halts the run, writes an `alert` audit entry, and demotes the task type back to `near`. |

Promotion is earned, not automatic: 3 consecutive successful runs in `in` (5 in `near`) make a task type eligible; a human clicks **Promote** (the API also accepts `force: true`). Manual mode buttons always override. Every run, step, decision, and mode change is in the audit log (SQLite).

Build order is the same as promotion order: in-the-loop first, out-of-the-loop last. Do not start a new task type in `out`.

## Modes

- [in-the-loop/](in-the-loop/README.md) — approve every step.
- [near-the-loop/](near-the-loop/README.md) — auto, pauses on exceptions, pause/resume/kill.
- [out-of-the-loop/](out-of-the-loop/README.md) — fully automatic, guardrails, alert + demotion on failure.

## Quick start (local CD)

One command deploys locally — gate, build, replace, health-check, rollback on failure. This is the primary workflow.

```bash
./deploy.sh                      # check.py gate -> docker compose build loop-app:<git-sha> -> up -> curl health check
docker compose logs -f app       # logs
docker compose ps                # status
docker compose down              # stop (data stays in the loop-data volume)
```

Images are tagged with the git short SHA (`loop-app:<sha>`, plus `loop-app:latest`). On health-check failure [deploy.sh](deploy.sh) rolls back to the previous image automatically. Manual rollback: `IMAGE_TAG=<old-sha> docker compose up -d`. The stack is defined in [compose.yaml](compose.yaml); [deploy.sh](deploy.sh) is thin glue (gate + SHA tag + rollback) on top of it. Branch per task; merge to `main` first — `./deploy.sh` only from `main`.

Works with any Docker daemon (Colima, Docker Desktop, …); the scripts use whichever `docker context` is active — keep one active at a time and check with `docker info`. `docker volume rm loop-data` deletes all run history — don't run it to "clean up".

Bare-metal debugging only (no gate, no rollback):

```bash
python3 server.py     # http://localhost:8000  (stdlib only, nothing to install)
python3 check.py      # end-to-end check of all three modes + promotion + mocked OpenRouter
```

Optional auto-deploy on every commit (not installed by default): `printf '#!/bin/sh\n./deploy.sh\n' > .git/hooks/post-commit && chmod +x .git/hooks/post-commit`.

Exercise the loop: submit a `summarize` run (in-the-loop, approve each step), switch `classify` to near and watch it pause on low confidence and the risky step, then put any task in out and add `FAIL` to the input to see the alert and demotion.

## Tiny Software Factory

The UI is a tiny software factory (see [docs/tsf-clone.md](docs/tsf-clone.md), with the source transcript): lists on the left (task types, crons, pages), focus in the center (selected run or page), the durable agent on the right (tools, core-four config, proposals). Close the browser and cron prompts keep creating runs; reopen it and the work is waiting for review. Deliberately DONE WRONG, like the lesson: one agent, cron in the server, one JSON config, SQLite on one box — each limit is marked `shortcut:` in [server.py](server.py).

## Build a real task

Edit `TEMPLATES` in [server.py](server.py) (steps are `name, confidence, risky`). `run_step()` — signature `(step_name, text, confidence) -> (output, confidence)`, raising `RuntimeError` on failure — calls OpenRouter when configured, else a local stub with no spend. Setup: `cp .env.example .env`, set `OPENROUTER_API_KEY=sk-or-...` (from your OpenRouter balance), then `./deploy.sh`. Never commit `.env`.

| Setting | Default | Notes |
|---|---|---|
| `OPENROUTER_API_KEY` | — (stub) | Set = real model calls; unset = local stub, no spend. |
| `OPENROUTER_MODEL` | `openai/gpt-4o-mini` | Main model. |
| `OPENROUTER_ESCALATION_MODEL` | `openai/gpt-4o` | Used when template confidence < 0.7. |
| `OPENROUTER_MAX_TOKENS` | `500` | Response cap per step. |
| `LOOP_DB` | `loop.db` (local) / `/data/loop.db` (container) | SQLite state file. |
| `PORT` / `HOST` | `8000` / `localhost` | Container sets `HOST=0.0.0.0`. |
| `LOOP_TOKEN` | unset | When set, `/api` requires `X-Loop-Token`. |

The model must reply with only JSON `{"output": "...", "confidence": 0.0-1.0}`; any call or parse failure raises, so the loop's pause/alert handling engages. Input containing `FAIL` always simulates a step failure.

> With a key set, run inputs are sent to OpenRouter/third-party models — never send confidential input until a data policy is chosen.

## API

All endpoints are JSON under `/api`. When `LOOP_TOKEN` is set, send `X-Loop-Token: <secret>` (UI: `localStorage.setItem("loop_token", "<secret>")` then reload); unset = localhost dev only.

| Endpoint | Description |
|---|---|
| `GET /api/task-types` | List task types (mode, success/failure counters, promotion eligibility). |
| `POST /api/task-types` | Create `{name, template, mode}` — template must be `summarize` \| `classify` \| `report`. |
| `POST /api/task-types/:id/mode` | Set mode `{mode: in\|near\|out}`. |
| `POST /api/task-types/:id/promote` | Promote one level (requires eligibility, or `{force: true}`). |
| `GET /api/runs` | Latest 50 runs. |
| `POST /api/runs` | Start a run `{task_type_id, input}` (mode is taken from the task type). |
| `GET /api/runs/:id` | One run with its steps and audit trail. |
| `POST /api/runs/:id/decide` | `{action: approve\|reject, edited_output?}` for the step awaiting decision. |
| `POST /api/runs/:id/control` | `{action: pause\|resume\|kill}`. |
| `GET /api/audit` | Latest 100 audit entries. |
| `GET /api/tools` | Agent access parity manifest: every UI control as an agent tool, plus harness slices. |
| `GET /api/config` · `PUT /api/config` | Core four (context, model, prompt, tools) as one JSON config; `PUT` is human-direct. |
| `GET/POST /api/crons`, `DELETE /api/crons/:id` | Cron prompts: `{name, schedule_s, prompt, task_type_id}` — the agent works while you're away. |
| `GET/POST /api/pages`, `POST/DELETE /api/pages/:id` | Agent-written pages, shown sandboxed in the center (Focus) pane. |
| `GET/POST /api/proposals`, `POST /api/proposals/:id/decide` | Self-modification proposals; nothing applies until a human approves. Never from `out`. |
| `POST /api/factories` | Initialize + configure + specialize in one call (same code as `python3 server.py --init-factory factory.json`). Starts in `in`. |

## Data

State is one SQLite file (`loop.db` locally, `/data/loop.db` in the container, persisted in the external `loop-data` volume). Never edit the DB by hand; back up the volume before schema changes. On restart, stale live runs are marked `killed` — no runner threads survive a restart.

[check.py](check.py) is the gate: it starts its own server on port 18099 with a temp DB, mocks OpenRouter (no network, no spend), and exercises all three modes plus promotion. It must pass before every deploy — [deploy.sh](deploy.sh) runs it first.

## Learns

[basics/](basics/README.md) — reference guides and demos (Python, TypeScript, Rust, Docker, REST, WebSockets, MD3). Frozen as learns; the real app lives in this root.

```bash
bash basics/check.sh    # check every basics demo
```
