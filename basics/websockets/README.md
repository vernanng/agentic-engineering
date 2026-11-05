# WebSocket (WS) Basics

WebSocket keeps one connection open so both sides can send messages at any time. REST is "ask, get an answer, hang up"; WS is "stay on the line".

Not a language either — libraries exist for Python, TypeScript, and Rust.

## REST vs WebSocket

|  | REST | WebSocket |
|---|---|---|
| Connection | New request per call | One long-lived connection |
| Who starts | Client only | Client or server, any time |
| Data | JSON request/response | Small messages, JSON or binary |
| Good for | CRUD, forms, fetching data | Chat, live prices, games, notifications |

Handshake starts as HTTP (`ws://` or `wss://` for encrypted), then upgrades to the WS protocol on the same connection.

## When you need it — and when you don't

Need it only when the **server must push** without the client asking first:

- Chat, live feeds, multiplayer games, collaborative editing, dashboards that update every second.

Don't need it for normal pages, forms, or CRUD. Polling with REST (`fetch` every few seconds) is simpler and often enough — start there, add WS when polling hurts.

## Run the echo demo

```bash
cd basics/websockets
python3 -m venv .venv && source .venv/bin/activate
pip install websockets
python3 server.py   # terminal 1
python3 client.py   # terminal 2: sends "hello", gets "echo: hello"
```

See [server.py](server.py) and [client.py](client.py). In a browser, the client side is built in: `new WebSocket("ws://localhost:8765")`.
