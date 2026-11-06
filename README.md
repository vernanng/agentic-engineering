# Agentic Engineering

Real app under active development — **local first**. Rust, one binary, no Docker. Everything runs on this machine at `http://localhost:8000`.

A task earns autonomy: **in-the-loop >> near-the-loop >> out-of-the-loop**.

| Mode | Behaviour |
|---|---|
| `in` | Pauses before every step. Human must Approve, Approve with an edited output, or Reject (reject kills the run). |
| `near` | Runs alone. Pauses only on low confidence (below the live `conf_threshold`, default 0.7), a risky step, or a failure. On a step pause, Approve runs the step (or saves an edited output instead); on a failure pause, Approve skips the step. Reject kills the run. Pause / Resume / Kill from the run list. |
| `out` | No decision prompts. Guardrails: 60s run/decision timeout (checked between steps, not a hard cap on a model call), steps from the template allowlist, 1 MB body cap. A failure halts the run, writes an `alert` audit entry, and demotes the task type back to `near`. |

Promotion is earned through **Promote**, not automatic: 3 consecutive completed runs in `in` (5 in `near`) make a task type eligible; a human clicks **Promote** (the API also accepts `force: true`). Two limits: creating a task type in `out` or setting a mode directly bypasses eligibility, and a failed step a human skipped still counts as a completed run. Promotions and demotions are audited; direct mode sets are not yet. Every run, step, and decision is in the audit log (SQLite).

## Quick start

```bash
cargo test          # gate: boots the real binary, mocked models, no spend
git push origin main  # the only deploy: CD gate -> DB backup -> release build -> swap binary -> health check -> rollback on failure
```

There is no local deploy. A non-docs push to `main` deploys to the VM via GitHub Actions ([.github/workflows/cd.yml](.github/workflows/cd.yml), GitHub-hosted runner over SSH, then [scripts/deploy.sh](scripts/deploy.sh) under systemd); Markdown/`docs/**`-only pushes do not trigger it, and the same job can be run manually (`workflow_dispatch`). On success the app is live at `http://localhost:8000` on the VM (public `:8000` per `.env` `HOST`); on health-check failure CD rolls back to the previous binary if one exists, else stops and fails. Logs: `tail -f loop-app.log`. Stop: `systemctl stop loop-app`. Bare run without the gate: `cargo run --release`.

Binaries are kept per short git SHA in `releases/`; the previous binary is the rollback. Rollback swaps the binary only; the DB is not restored.

Exercise the loop: submit a `summarize` run (approve each step), switch `classify` to near and watch it pause on low confidence and the risky step, then put any task in out and add `FAIL` to the input to see the alert and demotion.

Detached CLI (same API, no browser): `cargo run --release --bin client -- [--json] status|feed|runs|send <task_type_id> "<input>"|vote <id> up|down` (`LOOP_URL` defaults to `http://localhost:8000`).

## Change settings

- UI: Agent panel → Thinking level → **Save**. Note: `thinking_level` is saved but not yet wired to the model; saving it changes no model behaviour. Proposals apply only after a decide Approve in the UI workflow — but that is a convention, not an enforced boundary: any holder of `LOOP_TOKEN` (or anyone, when it is unset) can `PUT /api/config` directly or self-approve a proposal.
- API: `curl -X PUT http://localhost:8000/api/config -H 'Content-Type: application/json' -d '{"thinking_level":"high","max_tokens":800}'`. Keys: `model`, `escalation_model`, `cheap_model`, `jev_model`, `max_tokens` (1–8000), `conf_threshold` (0–1), `system_prompt`, `thinking_level` (`low|medium|high|max`), `enabled_slices` (non-empty subset of `factory|coding|pages|factory-controls`), `context_limit` (100–100000), `news_sources` (list of URL strings).
- Boot env (`.env`, sourced by [scripts/run-app.sh](scripts/run-app.sh) under launchd and by the CD fallback path; see [.env.example](.env.example) and the full table in [AGENTS.md](AGENTS.md)): `OPENROUTER_API_KEY` (unset = local stub, no spend), `OPENROUTER_MODEL` (`openai/gpt-4o-mini`), `OPENROUTER_ESCALATION_MODEL` (`openai/gpt-4o`), `OPENROUTER_MAX_TOKENS` (`500`), `OPENROUTER_CODING_MODEL` (cheap tier; unset = falls back to `OPENROUTER_MODEL`), `JEV_MODEL` (`~typesafe/jev-latest`), `JEV_MOCK` (`1` = labelled dev mock), `LOOP_TOKEN` (when set, `/api` needs `X-Loop-Token: <secret>` or `Authorization: Bearer <secret>`), plus `PORT`/`HOST`/`LOOP_DB`/`JEV_DIR`/`OPENROUTER_API_BASE`. Never commit `.env`.

> With a key set, run inputs are sent to OpenRouter/third-party models — never send confidential input until a data policy is chosen.

## Add a feature

1. Branch. Edit `templates()` or `run_step()` in [src/main.rs](src/main.rs) (steps are `name, confidence, risky`; keep the `AppResult<(output, confidence, usage)>` return; error on failure). UI in [static/index.html](static/index.html); new endpoints also join the hand-maintained `tools_manifest()` and the router in `handle()` — update both.
2. Extend [tests/gate.rs](tests/gate.rs). `cargo test` must pass.
3. Merge to `main` and push — Actions deploys it (docs-only pushes do not trigger CD). There is no local deploy.

## API

All endpoints are JSON under `/api`. When `LOOP_TOKEN` is set, send `X-Loop-Token: <secret>` (or `Authorization: Bearer <secret>`); unset = no auth, localhost dev only.

| Endpoint | Description |
|---|---|
| `GET/POST /api/task-types`, `POST /api/task-types/:id/mode`, `POST /api/task-types/:id/promote` | Task types, modes, earned promotion. |
| `GET/POST /api/runs`, `GET /api/runs/:id`, `POST /api/runs/:id/decide`, `POST /api/runs/:id/control`, `DELETE /api/runs/:id` | Runs, decisions, pause/resume/kill, delete finished runs. |
| `GET /api/audit` | Latest 100 audit entries. |
| `GET /api/tools`, `GET/PUT /api/config` | Agent parity manifest (hand-maintained; `handle()` is the source of truth); core-four config (PUT takes the shared token, no separate human check). |
| `GET/POST /api/crons`, `DELETE /api/crons/:id` | Cron prompts — the agent works while you're away. |
| `GET/POST /api/pages`, `GET/POST/DELETE /api/pages/:id` | Agent-written pages, sandboxed in the UI. |
| `GET/POST /api/proposals`, `POST /api/proposals/:id/decide` | Self-modification; applies on decide Approve. Never proposed from `out`. |
| `POST /api/factories` | Initialize + configure + specialize (same as `loop-app --init-factory f.json`). Starts in `in`. |
| `POST /api/triggers/:task_type_id`, `POST /api/events` | Webhook; log events (4xx/5xx fast path, rest Jev-gated into an `incident` run). |
| `GET/POST /api/items`, `POST /api/items/:id/vote`, `GET/POST /api/weights` | News cards (Hype/Facts/Signal); votes move weights. |
| `GET /api/executions` | Working now (live runs + enabled crons, idle crons included); Jev + step token/cost ledger. |

## Data

One SQLite file, `loop.db` — the same file the Python version used; history carries over. The gate checks a copy of it only when the file is present locally; the check is skipped on a fresh checkout. Never edit it by hand. [cd.yml](.github/workflows/cd.yml) backs it up to `backups/` before every deploy.
