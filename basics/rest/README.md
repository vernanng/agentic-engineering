# REST Basics

REST is how most apps talk over HTTP: the client sends a request, the server sends one response, then the connection closes. Stateless — every request carries everything the server needs (usually an auth token).

Not a language. You can build a REST API in Python, TypeScript, or Rust, and ship it in Docker.

## The shape

```text
METHOD  /path        e.g.  GET /users/42
Headers: Authorization: Bearer <token>, Content-Type: application/json
Body (POST/PUT only): JSON
```

| Method | Means | Example |
|---|---|---|
| GET | Read | `GET /users/42` → one user |
| POST | Create | `POST /users` with JSON body → new user |
| PUT / PATCH | Replace / update | `PUT /users/42` |
| DELETE | Delete | `DELETE /users/42` |

Status codes tell the result: `200` ok, `201` created, `400` bad input, `401` not logged in, `404` not found, `500` server error.

## Call it

```bash
curl http://localhost:8000/users
curl -X POST http://localhost:8000/echo -H 'Content-Type: application/json' -d '{"hi": 1}'
```

From TypeScript:

```ts
const res = await fetch("http://localhost:8000/users");
const users = await res.json();
```

## When you need it

Almost always, if a frontend, mobile app, or another service needs your data. CRUD, forms, file upload, payments — REST is the default.

See [api.py](api.py): a zero-dependency server. Run `python3 api.py`, then curl it.

Official docs: [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110.html) · [MDN HTTP](https://developer.mozilla.org/en-US/docs/Web/HTTP)
