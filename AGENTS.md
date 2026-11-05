# Agentic Engineering — real app, local first
- Root is product (`server.py`, `check.py`, `compose.yaml`, `deploy.sh`); `basics/` frozen, never edit for product work. Local only (localhost:8000) — no cloud/CI without explicit ask.
- Fast path: `python3 check.py` (gate, must pass) -> `./deploy.sh` (compose build `loop-app:<git-sha>` -> up -> health-check -> auto-rollback). Logs: `docker compose logs -f app`. Stop: `docker compose down`.
- Loop: build `in` >> `near` >> `out` (out last). Promote after 3 clean `in` / 5 clean `near`; `out` failure alerts + demotes to `near`. Guardrails: 60s timeout, template allowlist only.
- Tasks: edit `TEMPLATES`, keep `run_step(name, text, conf)` signature, stdlib only. `run_step()` calls OpenRouter when `OPENROUTER_API_KEY` is set, else stub (no spend). Model returns JSON `{output, confidence}`; any call/parse failure raises so loop pause/alert handles it.
- Models (env, in `run_step()`): `OPENROUTER_MODEL` (default `openai/gpt-4o-mini`); escalate to `OPENROUTER_ESCALATION_MODEL` (default `openai/gpt-4o`) when template conf < 0.7. Cap via `OPENROUTER_MAX_TOKENS` (default 500). Gate (`check.py`) mocks OpenRouter and strips the key for its server — never spend in the gate.
- Data/secrets: state in `loop-data` volume (`/data/loop.db`) — never `docker volume rm loop-data`, never edit DB by hand; back up volume before schema changes. Secrets/model keys via env only, never commit; `LOOP_TOKEN` stays unset while localhost-only.
- Docker: one context only (Colima or Desktop) — verify `docker info` before deploy.
- shortcut: with a key set, run inputs are sent to OpenRouter/third-party models — never send confidential input until a data policy is chosen.
