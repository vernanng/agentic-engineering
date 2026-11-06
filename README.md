# Agentic Engineering

Real app under active development — **local first**. Rust, one binary, no Docker. Everything runs on this machine at `http://localhost:8000`.

A task earns autonomy: **in-the-loop >> near-the-loop >> out-of-the-loop**.

| Mode | Behaviour |
|---|---|
| `in` | Pauses before every step. Human must Approve, Approve with an edited output, or Reject (reject kills the run). |
| `near` | Runs alone. Pauses only on low confidence (< 0.7), a risky step, or a failure (approve = skip step, reject = kill run). Pause / Resume / Kill from the run list. |
| `out` | No prompts. Guardrails only: 60s run timeout, steps from the template allowlist. A failure halts the run, writes an `alert` audit entry, and demotes the task type back to `near`. |

Promotion is earned, not automatic: 3 consecutive successful runs in `in` (5 in `near`) make a task type eligible; a human clicks **Promote** (the API also accepts `force: true`). Every run, step, decision, and mode change is in the audit log (SQLite).

## Quick start

```bash
cargo test          # gate: boots the real binary, mocked models, no spend
git push origin main  # the only deploy: CD gate -> release build -> DB backup -> swap binary -> health check -> rollback on failure
```

There is no local deploy. Push to `main` deploys on this machine via GitHub Actions ([.github/workflows/cd.yml](.github/workflows/cd.yml), self-hosted macOS runner). The app is live at `http://localhost:8000` after deploy. Logs: `tail -f loop-app.log`. Stop (launchd): `launchctl bootout "gui/$(id -u)/com.loop-app"`. Bare run without the gate: `cargo run --release`.

Binaries are kept per git SHA in `releases/`; the previous binary is the rollback.

Exercise the loop: submit a `summarize` run (approve each step), switch `classify` to near and watch it pause on low confidence and the risky step, then put any task in out and add `FAIL` to the input to see the alert and demotion.

Detached CLI (same API, no browser): `cargo run --release --bin client -- [--json] status|feed|runs|send <task_type_id> "<input>"|vote <id> up|down` (`LOOP_URL` defaults to `http://localhost:8000`).

## Change settings

- UI: Agent panel → Thinking level → **Save**. Proposals from agents apply only after you Approve them.
- API: `curl -X PUT http://localhost:8000/api/config -H 'Content-Type: application/json' -d '{"thinking_level":"high","max_tokens":800}'`. Keys: `model`, `escalation_model`, `cheap_model`, `jev_model`, `max_tokens` (1–8000), `conf_threshold` (0–1), `system_prompt`, `thinking_level`, `enabled_slices`, `context_limit`, `news_sources`.
- Boot env (`.env`, sourced by [scripts/run-app.sh](scripts/run-app.sh) under launchd and by the CD fallback path): `OPENROUTER_API_KEY` (unset = local stub, no spend), `OPENROUTER_MODEL` (`openai/gpt-4o-mini`), `OPENROUTER_ESCALATION_MODEL` (`openai/gpt-4o`), `DEEPSEEK_FLASH_MODEL` (cheap tier), `JEV_MODEL`, `LOOP_TOKEN` (when set, `/api` needs `X-Loop-Token`). Never commit `.env`.

> With a key set, run inputs are sent to OpenRouter/third-party models — never send confidential input until a data policy is chosen.

## Add a feature

1. Branch. Edit `templates()` or `run_step()` in [src/main.rs](src/main.rs) (steps are `name, confidence, risky`; keep the `(output, confidence, usage)` return; error on failure). UI in [static/index.html](static/index.html); new endpoints also join the `TOOLS` manifest so agents keep parity.
2. Extend [tests/gate.rs](tests/gate.rs). `cargo test` must pass.
3. Merge to `main` and push — Actions deploys it. There is no local deploy.

## API

All endpoints are JSON under `/api`. When `LOOP_TOKEN` is set, send `X-Loop-Token: <secret>`; unset = localhost dev only.

| Endpoint | Description |
|---|---|
| `GET/POST /api/task-types`, `POST /api/task-types/:id/mode`, `POST /api/task-types/:id/promote` | Task types, modes, earned promotion. |
| `GET/POST /api/runs`, `GET /api/runs/:id`, `POST /api/runs/:id/decide`, `POST /api/runs/:id/control`, `DELETE /api/runs/:id` | Runs, decisions, pause/resume/kill, delete finished runs. |
| `GET /api/audit` | Latest 100 audit entries. |
| `GET /api/tools`, `GET/PUT /api/config` | Agent parity manifest; core-four config (PUT human-direct). |
| `GET/POST /api/crons`, `DELETE /api/crons/:id` | Cron prompts — the agent works while you're away. |
| `GET/POST /api/pages`, `POST/DELETE /api/pages/:id` | Agent-written pages, sandboxed in the UI. |
| `GET/POST /api/proposals`, `POST /api/proposals/:id/decide` | Self-modification; applies only on human approve. Never from `out`. |
| `POST /api/factories` | Initialize + configure + specialize (same as `loop-app --init-factory f.json`). Starts in `in`. |
| `POST /api/triggers/:task_type_id`, `POST /api/events` | Webhook; log events (4xx/5xx fast path, rest Jev-gated into an `incident` run). |
| `GET/POST /api/items`, `POST /api/items/:id/vote`, `GET/POST /api/weights` | News cards (Hype/Facts/Signal); votes move weights. |
| `GET /api/executions` | Agents working now; Jev + step token/cost ledger. |

## Data

One SQLite file, `loop.db` — the same file the Python version used; history carries over (verified by the gate against a copy). Never edit it by hand. [cd.yml](.github/workflows/cd.yml) backs it up to `backups/` before every deploy.
