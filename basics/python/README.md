# Python Basics

Interpreted, dynamically typed language. Readable first, fast to write. The default for scripts, backends, data, and AI.

## Run it

```bash
python3 --version
python3 hello.py        # run a file
python3                 # REPL: type code interactively, Ctrl-D to exit
pip install requests    # install a package (prefer a .venv, see below)
```

## Core syntax

```python
name = "Ada"            # variable, no type declaration needed
age: int = 36           # optional type hint
items = [1, 2, 3]       # list
user = {"id": 1}        # dict (like a JS object / JSON)
point = (3, 4)          # tuple, immutable

if age >= 18:
    print("adult")      # indentation IS the block — 4 spaces, no braces
elif age >= 13:
    print("teen")
else:
    print("kid")

for x in items:         # loop over values directly
    print(x)

def greet(who: str) -> str:   # function
    return f"hi {who}"

class Counter:          # class
    def __init__(self):
        self.n = 0
    def inc(self):
        self.n += 1
```

Key ideas:

- Everything is an object; types are checked at runtime (`dynamic`).
- `None` is null. Truthiness: `0`, `""`, `[]`, `{}` are falsy.
- Comprehension: `squares = [x * x for x in range(5)]`.
- Errors are exceptions: `try: ... except ValueError as e: ... finally: ...`.

## Project layout

```bash
python3 -m venv .venv && source .venv/bin/activate  # isolated env
pip install flask
pip freeze > requirements.txt   # pin deps; reinstall: pip install -r requirements.txt
```

See [hello.py](hello.py) and run it.

Official docs: [Python Tutorial](https://docs.python.org/3/tutorial/)
