# Jev commands

Cloned from [disler/pi-durable-news-agent](https://github.com/disler/pi-durable-news-agent)
(MIT, © the pi-durable-news-agent authors), `apps/dna-server-api/prompt_engineering/jev/`.
Each `<name>/command.json` is one Jev command: a `state` template, typed
`questions` (`noul` / `choice` / `score`), and a `policy` block. `{{var}}`
must be a whole value; `each` expands a list into one question per item.

Run by `jev_classify()` in [server.py](../../server.py) against the real Jev
model on OpenRouter (`POST https://openrouter.ai/api/alpha/decisions`,
model `~typesafe/jev-latest` by default — upstream's own default provider
path). Code decides thresholds; Jev decides judgments.

| Command | Gate in the sweep |
|---|---|
| `relevance` | Is this item about the reader's weighted topics? (min 0.45) |
| `worth-attention` | Worth an agentic engineer's attention today? (min 0.35) |
| `importance` | Rank gated items; top 7 are presented |
| `same-story` | Dedupe fallback when the content hash misses |
| `claim-support` | Does the evidence support each claim? (min 0.7) |
| `hfs` | Hype / Facts / Signal scores, 0–100 each |
