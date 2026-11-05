# Tiny Software Factory clone — logic map

Source: IndyDevDan, *Tiny Software Factories: Agentic Engineering DONE WRONG*
(`1v7ldJiyH-Y`, 18:36). Full auto-generated transcript: [tsf-transcript.txt](tsf-transcript.txt)
(499 snippets, 00:00–18:26; last ~10s is outro music). Auto-captions are
machine-generated, so numbers below are directional, not spec.

Every row traces to a transcript timestamp and names the surface that
implements it in this repo. "Existing" means the loop app already had it;
"New" is added by this change. Anti-patterns are kept on purpose (that is
the lesson) and marked with a `shortcut:` note where each one breaks.

| # | Video logic (timestamp) | Surface here | Status |
|---|---|---|---|
| 1 | One problem, one proactive agent, around the clock, durable environment, human near the loop not in it [02:16] | Task types + `runner()` in [server.py](../server.py); modes in [README.md](../README.md) | Existing |
| 2 | In loop you prompt; near loop you respond. Feedback is a data flywheel [03:00] | `decisions` + `audit` tables; `consecutive_success` promotion counters | Existing |
| 3 | 80%+ of the work can run without us with the right system [03:21] | `near`/`out` modes, earned promotion 3 clean `in` / 5 clean `near` | Existing |
| 4 | Initialize, configure, specialize: one meta skill + one command mounts a factory next to your codebase, zero to working in minutes [03:48][04:56] | `POST /api/factories` + `python3 server.py --init-factory factory.json`; new factories always start `in` | New |
| 5 | Empty factory: fully built template, every feature, no specialization yet [06:24] | Seeded `summarize|classify|report` task types, no runs | Existing |
| 6 | Agent on your right: durable agent, always on, one message away [07:06] | Right agent panel in [static/index.html](../static/index.html); agent tools under `/api/tools` | New UI, existing runner |
| 7 | Lists on your left: every product is a list of something [07:41] | Left lists: task types, runs, crons, pages | New UI layout |
| 8 | Focus in the center: generative UI — pages/artifacts/markdown the agent builds, tweaks, tunes [08:31] | `pages` table + `/api/pages`; center pane renders the selected run or page | New |
| 9 | Agent access parity: everything you can do, your agent can do; every UI control has a matching agent tool [09:15] | `GET /api/tools` manifest; tools call the same handlers as the UI | New |
| 10 | Core four control: context, model, prompt, tools [09:56] | `GET/PUT /api/config` (single JSON settings row); `run_step()` reads it live | New |
| 11 | Context: auto compaction at 200k, force at 500k [10:32] | `context_limit` in config truncates run input in `run_step()` | New (threshold scaled to this app) |
| 12 | Model: tiered selection + thinking level, right model for the job, never fixate on one model [10:46] | `model` / `escalation_model` / `thinking_level` in config; existing OpenRouter escalation below `conf_threshold` | Existing + config |
| 13 | Prompt: prompt-engineered system prompt, custom + extension prompts appendable [10:58][11:19] | `system_prompt` in config, prepended to every step prompt | New |
| 14 | Tools in view: list, descriptions, full parameters, quick view [11:30] | `GET /api/tools` + tools list in the agent panel | New |
| 15 | Self-modification via parity: "disable all coding tools, update thinking level to max" — reflection for agents, dangerous without control [11:49] | `POST /api/proposals` + `POST /api/proposals/:id/decide`; applies only on human approve, never from `out` | New, gated |
| 16 | Extensible harness: agent is a composition of capability slices — factory, coding, pages, image generation, factory controls [12:49] | `SLICES` registry + `enabled_slices` in config; disabling `coding` makes `run_step()` raise so pause/alert handling engages | New (image generation slice not cloned — no image surface in this app) |
| 17 | Customizable DX/UX: themes, nav bars, hotkeys, animations — liquid software [13:55] | Existing MD3 light/dark roles; three-region layout. Hotkeys/animations not cloned (no ask, low value here) | Partial |
| 18 | Cron prompts: name + schedule + prompt; agent works while you're away; browser can close, agent keeps working [16:10][17:56] | `crons` table + scheduler thread in the server + `/api/crons` | New |
| 19 | TSF is near-loop and deliberately lossy; IA3/out-loop is the scalable version [01:01][01:11] | This clone stays near-loop; `out` keeps guardrails only (60s timeout, allowlist, alert + demote) | Existing stance |

## Kept anti-patterns (DONE WRONG, on purpose)

- One agent, one process: the scheduler thread dies with the server.
  shortcut: cron state is durable in SQLite and `next_run_at` is
  recomputed on boot, so a restart delays but never loses a cron.
- Cron inside the server, JSON config on one box, SQLite single file:
  same limits as the existing `shortcut:` notes in [server.py](../server.py).
- Agent-written pages are stored text rendered in a sandboxed frame;
  if sandboxing is ever removed, stored pages become a stored-XSS hole.
