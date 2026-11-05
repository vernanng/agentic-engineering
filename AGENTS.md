# Agentic Engineering
- Real app is this root (`server.py`, `check.py`); `basics/` is frozen learns, do not edit it for product work.
- Build order: in-the-loop >> near-the-loop >> out-of-the-loop; out last.
- Loop rules: promote after 3 clean `in` runs (5 in `near`); `out` failure alerts and demotes to `near`. Guardrails: 60s timeout, template allowlist only.
- Real tasks: edit `TEMPLATES` in `server.py`, keep `run_step(name, text, conf)` signature. Stdlib only.
- Check: `python3 check.py`.
- Models (planned, not wired yet — `run_step()` is still canned): default `muse-spark-1.3-contributor` (`summarize`, `in`, conf >= 0.7, not risky). Escalate to `muse-wonder-1.0-contributor` on risky, conf < 0.7 pause, failure, or 2 pauses in one run. Wire this inside `run_step()` before claiming it is enforced.
- shortcut: Contrib trains on inputs; never use Contrib in `out` or with confidential input, use Standard there.
