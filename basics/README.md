# Basics

Six topics. The first four are tools; REST and WebSocket are ways for programs to talk to each other over a network.

| Tool | Kind | Typing | Runs as | Use it for |
|---|---|---|---|---|
| Python | Language | Dynamic, optional hints | Interpreted | Scripts, APIs, data, AI |
| TypeScript | Language (superset of JS) | Static, inferred | Compiled to JS | Web apps, Node APIs |
| Rust | Language | Static, strict | Compiled to native | Fast, safe systems code |
| Docker | Packaging tool | — | Containers | Shipping apps identically anywhere |
| REST | API style over HTTP | — | Request → response | Frontend/mobile talking to a backend |
| WebSocket | Protocol over HTTP upgrade | — | Open connection, both sides push | Chat, live feeds, games |

## Folders

- [python/](python/README.md) — start here if you have never coded.
- [typescript/](typescript/README.md) — JavaScript with types, for web/Node.
- [rust/](rust/README.md) — performance + memory safety without a garbage collector.
- [docker/](docker/README.md) — package any of the above so it runs the same on every machine.
- [rest/](rest/README.md) — how clients fetch and change data on a server. Needed for almost any web app.
- [websockets/](websockets/README.md) — live, two-way updates. Only needed when the server must push (chat, live feeds, games).
- [loop-app/](loop-app/README.md) — runnable app built from the above: a task earns autonomy, in-the-loop >> near-the-loop >> out-of-the-loop.
- [md3/](md3/README.md) — Material Design 3: colour roles, type/shape scales, and standard components, with a demo page.

## How they combine

A typical stack: write the app in Python, TypeScript, or Rust, expose it over REST (and WebSocket only if it needs live push), then ship it in Docker.

```text
your code  ->  Dockerfile  ->  image  ->  container (runs anywhere)
```
