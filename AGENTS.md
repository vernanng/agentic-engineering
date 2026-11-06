# Agentic Engineering — real app, local first

> Agent operating rules for this repo. Shared product facts live in [README.md](README.md) — do not duplicate them here.
> Local only (`http://localhost:8000`). No cloud without an explicit ask. No Docker.

## Project layout

| Path | What it is |
| --- | --- |
| [src/main.rs](src/main.rs) | Rust app — single binary `loop-app` |
| [src/bin/client.rs](src/bin/client.rs) | Detached CLI `client` (`LOOP_URL`, default `http://localhost:8000`) |
| [static/index.html](static/index.html) | UI |
| [tests/gate.rs](tests/gate.rs) | Gate tests |
| [scripts/](scripts/run-app.sh) | launchd entry point (`run-app.sh`) and service (`loop-app.plist`, label `com.loop-app`) |
| [.github/workflows/cd.yml](.github/workflows/cd.yml) | CD — the only deploy: gate → build → backup → restart → health-check on push to `main` |

## Quick start

```sh
cargo test        # gate — must pass
git push origin main   # the only deploy — CD runs it, see Deployment
```

- Logs: `tail -f loop-app.log`
- Restart (launchd, the normal case): `launchctl kickstart -k "gui/$(id -u)/com.loop-app"`
- Stop (launchd): `launchctl bootout "gui/$(id -u)/com.loop-app"` — a plain `kill` does not stop it; launchd `KeepAlive` respawns it.
- Stop (no launchd service loaded): `kill "$(cat .loop-app.pid)"`

## Git workflow

- One branch per task. **Never code directly on `main`.**
- Merge to `main` first, then deploy.
- Push to `main` is the only deploy: it triggers [cd.yml](.github/workflows/cd.yml) on the self-hosted runner (macOS). There is no local deploy.
- **Never push without an explicit ask.**

## Running locally

Bare run (no gate, no rollback):

```sh
cargo run --release
```

Secrets live in `.env` only (gitignored — see [.env.example](.env.example)). [scripts/run-app.sh](scripts/run-app.sh) (launchd) and the CD fallback path source `.env`. Bare `cargo run` does **not**.

| Variable | Default / notes |
| --- | --- |
| `PORT` | `8000` |
| `HOST` | `localhost` |
| `LOOP_DB` | `loop.db` |
| `LOOP_TOKEN` | unset = no auth (localhost dev); set = every `/api` call needs `X-Loop-Token: <secret>` (or `Authorization: Bearer <secret>`) |
| `LOOP_URL` | `http://localhost:8000` — CLI (`client`) only |
| `OPENROUTER_API_KEY` | unset = local stub, no spend; set = calls OpenRouter |
| `OPENROUTER_MODEL` | `openai/gpt-4o-mini` |
| `OPENROUTER_ESCALATION_MODEL` | `openai/gpt-4o` |
| `OPENROUTER_MAX_TOKENS` | `500` |
| `DEEPSEEK_FLASH_MODEL` | unset = cheap tier falls back to `OPENROUTER_MODEL` |
| `JEV_MODEL` | `~typesafe/jev-latest` |
| `JEV_MOCK` | unset = off; `1` = labelled dev mock |
| `JEV_DIR` | unset = `prompt_engineering/jev/` |
| `OPENROUTER_API_BASE` | `https://openrouter.ai` — test override only |

## Deployment

There is no local deploy. Push to `main` triggers [.github/workflows/cd.yml](.github/workflows/cd.yml) on the self-hosted macOS runner, which pulls `main` into this checkout and performs, in order:

1. Gate (`cargo test`)
2. `cargo build --release`
3. DB backup to `backups/`
4. Install binary to `releases/loop-app-<git-sha>` (recorded in `.loop-app.current`)
5. Restart: `launchctl kickstart -k` when the `com.loop-app` launchd service is loaded (entry [scripts/run-app.sh](scripts/run-app.sh)), else kill the pid in `.loop-app.pid` and relaunch with `nohup`
6. Health-check `GET /api/task-types`
7. Auto-rollback to the previous binary on failure

## The loop

Build `in` >> `near` >> `out`. `out` is last — never start a new task type in `out`.

| From | Requirement |
| --- | --- |
| `in` | 3 consecutive clean runs |
| `near` | 5 consecutive clean runs |

- Promotion is a manual Promote click. `force:true` bypasses the count.
- `out` failure: halts the run, writes an `alert` audit entry, and demotes the task type to `near`.
- Guardrails: 60s run/decision timeout, template allowlist only (`summarize | classify | report | incident | news`), 1 MB body cap.

## Tasks & templates

- Edit `templates()` in [src/main.rs](src/main.rs). Steps are `(name, confidence, risky)`.
- Keep the signature: `run_step(step_name, text, confidence) -> (output, confidence, usage)`
- Test hook: input containing `FAIL` always errors (failure path).

## Executor & models

Entry point: `run_step()` in [src/main.rs](src/main.rs).

- If `OPENROUTER_API_KEY` is set → calls OpenRouter. Otherwise → local stub (no spend).
- Tiering: confidence ≥ 0.9 → cheap (`cheap_model`); mid-range → main model; below live `conf_threshold` (default `0.7`) → escalation.
- Usage: real `usage` (tokens, cost) is stored on the step when present. Without it, the chars/4 proxy with cost `0` is kept.
- Contract: the model must return JSON `{"output", "confidence"}`. Any call/parse failure errors, so pause/alert handling engages.
- Config is read live via `get_config()`.

## API

All routes are `/api/*` and speak JSON. The full route list is not copied here: use `GET /api/tools` (the `TOOLS` manifest in [src/main.rs](src/main.rs)) — it cannot drift from the code.

- Auth: `LOOP_TOKEN` set → send `X-Loop-Token: <secret>` (or `Authorization: Bearer <secret>`). Unset → no auth, localhost dev only.
- `GET /api/runs` returns the latest 50; `GET /api/audit` the latest 100. `DELETE /api/runs/:id` is finished runs only.
- `POST /api/events`: 4xx/5xx fast path, else Jev-gated.
- `POST /api/factories` = `loop-app --init-factory f.json`; always starts `in`.
- **Jev** commands live in `prompt_engineering/jev/` (`relevance | worth-attention | importance | same-story | hfs`). No key fails closed. `JEV_MOCK=1` is the labelled dev mock.
- **`news`** runs are deterministic sweeps (gather → gate → dedupe → rank → present, top 7). Feed reads are capped at 5 MB.
- **On boot:** `queued` runs resume; mid-flight runs are killed.
- Disabling the `coding` slice makes `run_step()` error.

## Data

- One SQLite file: `loop.db` (same schema as the Python era; old DBs are migrated in place with `tokens`/`cost` columns).
- **Never edit the DB by hand.** [cd.yml](.github/workflows/cd.yml) backs it up before every deploy.
- Pre-Rust backups: `backups/loop-local-pre-rust.db`, `backups/loop-docker-pre-rust.db`.

## Gate (tests)

```sh
cargo test
```

- Boots the real binary on port `18099` with a temp DB.
- Mocks OpenRouter + Jev locally — no network, no spend.
- Covers: all modes, promotion, tools/config/pages/proposals/slices/cron/factory, triggers, the sweep, the CLI, and reading a copy of the real `loop.db`.

## Known shortcuts

<!-- Keep each shortcut line at the start of its bullet, verbatim.
     Tooling greps for them — do not reword or bury them in tables. -->

- shortcut: with a key set, run inputs are sent to OpenRouter/third-party models — never send confidential input until a data policy is chosen.
- shortcut: [src/main.rs](src/main.rs) uses one SQLite connection + lock for all runs; upgrade to per-thread connections when parallel `out` runs matter.
- shortcut: cron lives in this process and dies with it; `next_run_at` is durable in SQLite, so a restart delays, never loses.
