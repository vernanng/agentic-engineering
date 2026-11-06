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
| [.github/workflows/cd.yml](.github/workflows/cd.yml) | CD — the only deploy: gate → backup → build → restart → health-check on push to `main` |

## Quick start

```sh
cargo test        # gate — must pass
git push origin main   # the only deploy — CD runs it, see Deployment
```

- Logs: `tail -f loop-app.log` (or `journalctl -u loop-app`)
- Restart (systemd on the VM): `systemctl restart loop-app`
- Stop: `systemctl stop loop-app` — a plain `kill` does not stop it; systemd `Restart=on-failure` respawns it. A bare `cargo run` is separate: kill that process directly.

## Git workflow

- One branch per task. **Never code directly on `main`.**
- Merge to `main` first, then deploy.
- Push to `main` is the only deploy: a non-docs push triggers [cd.yml](.github/workflows/cd.yml) on a GitHub-hosted runner, which SSHes to the VM (this checkout) and runs [scripts/deploy.sh](scripts/deploy.sh); `**.md`/`docs/**`-only pushes are ignored, and the same job can be dispatched manually. There is no local deploy.
- **Never push without an explicit ask.**

## Running locally

Bare run (no gate, no rollback):

```sh
cargo run --release
```

Secrets live in `.env` only (gitignored — see [.env.example](.env.example)). [scripts/run-app.sh](scripts/run-app.sh) (systemd entry point) sources `.env`. Bare `cargo run` does **not**.

| Variable | Default / notes |
| --- | --- |
| `PORT` | `8000` |
| `HOST` | `localhost` |
| `LOOP_DB` | `loop.db` |
| `LOOP_TOKEN` | unset = no auth (localhost dev); set = every `/api` call needs `X-Loop-Token: <secret>` (or `Authorization: Bearer <secret>`) |
| `LOOP_URL` | `http://localhost:8000` — CLI (`client`) only |
| `TURSO_DATABASE_URL` / `TURSO_AUTH_TOKEN` | unset = local `LOOP_DB` file; both set = Turso remote DB (spike backend in `src/main.rs`) |
| `OPENROUTER_API_KEY` | unset = local stub, no spend; set = calls OpenRouter |
| `OPENROUTER_MODEL` | `openai/gpt-4o-mini` |
| `OPENROUTER_ESCALATION_MODEL` | `openai/gpt-4o` |
| `OPENROUTER_MAX_TOKENS` | `500` |
| `OPENROUTER_CODING_MODEL` | unset = cheap tier falls back to `OPENROUTER_MODEL` |
| `JEV_MODEL` | `~typesafe/jev-latest` |
| `JEV_MOCK` | unset = off; `1` = labelled dev mock |
| `JEV_DIR` | unset = `prompt_engineering/jev/` (else next to the binary) |
| `OPENROUTER_API_BASE` | `https://openrouter.ai` — base-URL override; the gate points it at its mock, a live value redirects real calls |

## Deployment

There is no local deploy. A non-docs push to `main` triggers [.github/workflows/cd.yml](.github/workflows/cd.yml) on a GitHub-hosted runner, which SSHes to the VM (secrets `VM_HOST`/`VM_USER`/`VM_SSH_KEY`), pulls `main` into this checkout (`/root/project/agentic-engineering`) and runs [scripts/deploy.sh](scripts/deploy.sh), which performs, in order:

1. Gate (`cargo test --release --locked --test gate`)
2. DB backup to `backups/`
3. `cargo build --release`
4. Install binary to `releases/loop-app-<short-sha>` (recorded in `.loop-app.current`, symlinked as `releases/loop-app-latest`)
5. Restart: `systemctl restart loop-app` (unit [scripts/loop-app.service](scripts/loop-app.service), entry [scripts/run-app.sh](scripts/run-app.sh); binds `HOST` from `.env`, currently `0.0.0.0:8000`)
6. Health-check `GET /api/task-types`
7. Auto-rollback to the previous binary on failure — only when a previous binary exists, else stop and fail. The DB is not restored.

`LOOP_TOKEN`, `OPENROUTER_API_BASE`, `TURSO_DATABASE_URL` and `TURSO_AUTH_TOKEN` are managed in GitHub secrets: [cd.yml](.github/workflows/cd.yml) pipes them to the VM as `.env.cd` on each deploy and [scripts/merge-env.sh](scripts/merge-env.sh) merges the non-empty ones into `.env` (self-check: `bash scripts/test-merge-env.sh`). Edit them in GitHub, not in `.env` — hand edits are overwritten on the next deploy. All other `.env` keys stay VM-only.

## The loop

Build `in` >> `near` >> `out`. `out` is last — never start a new task type in `out` by policy; the API does not yet enforce it (direct create/mode-set bypass, see Follow-ups).

| From | Requirement |
| --- | --- |
| `in` | 3 consecutive completed runs |
| `near` | 5 consecutive completed runs |

- Promotion is a manual Promote click. `force:true` bypasses the count. A failed step skipped by a human still counts as a completed run.
- `out` failure: halts the run, writes an `alert` audit entry, and demotes the task type to `near`.
- Guardrails: 60s run/decision timeout (checked between steps; a paused run and a `news` sweep have no timeout yet), template allowlist only (`summarize | classify | report | incident | news`), 1 MB body cap.

## Tasks & templates

- Edit `templates()` in [src/main.rs](src/main.rs). Steps are `(name, confidence, risky)`.
- Keep the signature: `run_step(step_name, text, confidence) -> AppResult<(output, confidence, usage)>`
- Test hook: input containing uppercase `FAIL` errors when the step actually executes. An edited-output approval and a `news` sweep never reach `run_step()`, so neither triggers it.

## Executor & models

Entry point: `run_step()` in [src/main.rs](src/main.rs).

- If `OPENROUTER_API_KEY` is set → calls OpenRouter. Otherwise → local stub (no spend).
- Tiering (`model_for`, escalation checked first): below live `conf_threshold` (default `0.7`) → escalation; else confidence ≥ 0.9 → cheap (`cheap_model`); else main model. With a threshold above 0.9, the cheap tier is unreachable below it.
- Usage: real `usage` (tokens, cost) is stored on the step when present. Without it, the UTF-8 bytes/4 proxy with cost `0` is kept.
- Contract: the model must return JSON `{"output", "confidence"}`. Any call/parse failure errors, so pause/alert handling engages.
- Config is read live via `get_config()`. Exception: `thinking_level` is stored and validated but not yet read by the executor.

## API

All routes are `/api/*` and speak JSON. The full route list is not copied here: `GET /api/tools` serves the hand-maintained `tools_manifest()` in [src/main.rs](src/main.rs); the router in `handle()` is the source of truth when they differ.

- Auth: `LOOP_TOKEN` set → send `X-Loop-Token: <secret>` (or `Authorization: Bearer <secret>`). Unset → no auth, localhost dev only. One shared token covers every route; there is no separate human check on `PUT /api/config` or proposal decide.
- `GET /api/runs` returns the latest 50; `GET /api/audit` the latest 100. `DELETE /api/runs/:id` is finished runs only.
- `POST /api/events`: 4xx/5xx fast path, else Jev-gated.
- `POST /api/factories` shares `create_factory` with `loop-app --init-factory f.json` and starts `in`; the CLI path also boots the DB first, which kills mid-flight runs.
- **Jev** commands live in `prompt_engineering/jev/` (`relevance | worth-attention | importance | same-story | hfs`). No key fails closed. `JEV_MOCK=1` is the labelled dev mock.
- **`news`** runs follow a fixed pipeline (gather → gate → dedupe → rank → present, top 7); scores come from live Jev, so results are not deterministic. Feed reads are capped at 5 MB.
- **On boot:** `queued` runs resume; running/awaiting/paused runs are killed and their pending steps rejected.
- Disabling the `coding` slice makes `run_step()` error. `news` sweeps do not call `run_step()` and are unaffected.

## Data

- One SQLite file: `loop.db` (same schema as the Python era; old DBs are migrated in place with `tokens`/`cost` columns).
- **Never edit the DB by hand.** [cd.yml](.github/workflows/cd.yml) backs it up before every deploy.
- Pre-Rust backups: `backups/loop-local-pre-rust.db`, `backups/loop-docker-pre-rust.db`.

## Gate (tests)

```sh
cargo test
```

- Same gate runs on GitHub-hosted CI ([ci.yml](.github/workflows/ci.yml), cached debug build) for branches and PRs; CD re-runs it in release on the VM (via [scripts/deploy.sh](scripts/deploy.sh)).
- Boots the real binary on port `18099` with a temp DB.
- Mocks OpenRouter + Jev locally — no network, no spend.
- Covers: all modes, promotion, tools/config/pages/proposals/slices/cron/factory, triggers, the sweep, and the CLI. It reads a copy of the real `loop.db` only when that file is present locally; a fresh checkout skips that check. The whole gate is one sequential `#[test]`.

## Known shortcuts

<!-- Keep each shortcut line at the start of its bullet, verbatim after the prefix.
     A `shortcut:` grep should find the same text here and in src/main.rs. -->

- shortcut: one connection + lock serialises all runs, upgrade to per-thread connections when parallel out runs matter
- shortcut: cron lives in this process and dies with it; next_run_at is durable in SQLite and recomputed on boot, so a restart delays, never loses.
- shortcut: feeds over 5 MB fail parse and are skipped per-source, raise the cap when a real source outgrows it
- shortcut: jev same-story only when titles collide; hash+title cover the common case

Note: with a key set, run inputs are sent to OpenRouter/third-party models — never send confidential input until a data policy is chosen.

## Follow-ups

Code gaps the docs disclose but do not fix. One line each; fix in code, then update the doc claim.

- Enforce earned promotion: reject direct `out` creation and mode sets that skip eligibility (or rename them as an explicit admin override).
- Do not count runs with a skipped failed step toward promotion, or rename the counter to completed runs in the UI.
- Add a real approver check for `PUT /api/config` and proposal decide, or fix the in-app `tools_manifest()` wording that claims human-only.
- Wire `thinking_level` into the model call, or remove the setting from config and UI.
- Audit direct mode changes (`POST /api/task-types/:id/mode` writes no audit entry today).
- Add `GET /api/pages/:id` to `tools_manifest()`, or generate the manifest from the router.
- Install/load the launchd service in setup or CD, or document the manual install wherever restart is assumed.
- Give `news` sweeps and paused runs a timeout (only the step runner and decision waits have one today).
