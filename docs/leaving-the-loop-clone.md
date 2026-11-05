# Leaving the loop + durable news agent — logic map (part 2)

Sources: IndyDevDan, *ELITE Agentic Engineers Are LEAVING THE LOOP*
(`52JX83rGPLc`, 25:46; [transcript](leaving-the-loop-transcript.txt), 779
snippets) and *My Jev Pi Durable Agent CURATES News* (`OLqP8jxJ0Wc`, 22:34;
[transcript](news-agent-transcript.txt), 668 snippets), plus the upstream
codebase [disler/pi-durable-news-agent](https://github.com/disler/pi-durable-news-agent)
(MIT). Part 1 (Tiny Software Factories) is in [tsf-clone.md](tsf-clone.md).
Auto-captions are machine-generated; numbers are directional.

| # | Logic (source) | Surface here | Status |
|---|---|---|---|
| 1 | Prompt → Click → Cron: in you prompt, near you click/respond, out is full cron (`52J` [02:25]) | `fire_trigger()` in [server.py](../server.py): human start, `decide`, cron, webhook, events all enter through `start_run()` | New |
| 2 | Proactive = durable agent + trigger in a sandbox; it works for you, then waits (`52J` [07:38], `OLq` [03:02]) | Triggers (cron/webhook/event) + `near` pause = the wait | Existing + triggers new |
| 3 | Triggers are events, not you: webhook, cron, log stream, tickets, UI actions, CLI (`52J` [04:24][07:15]) | `POST /api/triggers/:task_type_id`, `POST /api/events`, `client.py` | New |
| 4 | Incident workflow: Detect → Triage → Patch → Ask → Apply → Verify, results saved (`52J` [05:30]) | `incident` template; the risky `apply fix` step in `near` is the Ask | New |
| 5 | 4xx/5xx need no classifier; Jev classifies the rest, fast (`52J` [09:50]) | `POST /api/events` fast path; Jev `relevance` for the rest | New |
| 6 | Cheap Jev in front of expensive models at every gate (`OLq` [04:06][10:24]) | `jev_classify()` → OpenRouter Decisions API, before any chat call | New, real Jev |
| 7 | Sweep: gather → gate → dedupe → rank → present, top 7 (`OLq` [10:07], upstream sweep table) | `news` template run by `do_sweep()`; cron on it is the tick | New |
| 8 | Hype / Facts / Signal: score all three, never pick one (`OLq` [05:19], upstream H/F/S) | Jev `hfs` command; 0–100 per bucket on every card | New |
| 9 | Dedupe: content hash first, Jev `same-story` as fallback (`OLq` [10:54]) | Hash fold in `do_sweep()`, then Jev | New |
| 10 | Taste flywheel: votes move weights, labels move keywords, agent edits weights (Gemini 1.2→1.3) (`OLq` [12:29][13:53]) | `POST /api/items/:id/vote`, `weights` table, proposals for agent edits | New |
| 11 | Hype+signal without facts is suspect → ask the human (upstream H/F/S) | Suspect cards open a `proposals` question | New |
| 12 | Detached clients, one API: browser, TUI, CLI synced on SQLite state (`OLq` [02:19][08:30]) | [client.py](../client.py) over the same `/api/*` capability list | New |
| 13 | Durability: resume from checkpoint after crash/restart, never pay twice (`OLq` upstream Durability) | Boot resumes `queued` runs; `done` steps are checkpoints | New |
| 14 | Measure leverage: how many agents are working for you right now (`52J` [21:00]) | Header count: live runs + enabled crons; Executions view with tokens/cost | New |
| 15 | Core four multiplied: context, tokenomics, prompt, harness (`52J` [15:38]) | Core-four config (part 1) + per-step tokens/cost + Jev ledger | Existing + ledger new |

## Not cloned (named non-goals)

- Deep Jev UI Replicator (`52J` [12:03]): needs vision/screenshots; breaks
  stdlib-only. Its loop shape (score → threshold → iterate) lives on as
  rank-until-top-7.
- Pi / Pi-Durable / TypeSafe-direct hosting, exe.dev sandboxes, multiplayer,
  web search chains, generated card images: foreign stacks or paid extras.
  The sandbox remains this box + Docker.
- Real Jev *is* cloned (via OpenRouter, upstream's own default provider
  path) — see [prompt_engineering/jev/README.md](../prompt_engineering/jev/README.md).

## Verification evidence

- Gate: `python3 check.py` — jev wire (fake HTTP: decisions endpoint, model
  alias, 503-retry, fail-closed without key), queued-run resume, webhook
  trigger, incident Ask-at-apply, news sweep under the labelled mock
  (off-topic gated, hash dupe folded, top ≤7, H/F/S scored), vote→weight
  flywheel, CLI round-trip. All sections pass.
- Live Jev smoke (one real call, outside the gate): resolved model
  `typesafe/jev-1.14-20261014`, typed score answers, reported cost
  $0.0000169 for 403 input tokens.
- REA (summary-first, CLI fallback): `ev_66aa15fc401b7c438a80d2b6445e1adccbaa41795418ebceaa8fbf818cdcb772`
  over the extracted UI scripts — 2 files, 1,735 AST nodes, 0 parse failures.
  Static only; nothing executed.
