# TypeScript Basics

JavaScript plus static types. It compiles (transpiles) to plain JavaScript, so it runs anywhere JS runs: browsers and Node.js. Types are erased at runtime — they only help you catch bugs before running.

## Run it

```bash
node --version
npm init -y
npm install -D typescript tsx
npx tsc --init          # creates tsconfig.json
npx tsx hello.ts        # run directly (dev)
npx tsc                 # compile to dist/*.js, then: node dist/hello.js
```

## Core syntax

```ts
let name: string = "Ada";        // typed variable
let age = 36;                    // type inferred as number
const items: number[] = [1, 2, 3];

interface User { id: number; name?: string }  // shape; ? = optional
type Id = string | number;       // union type

function greet(who: string): string {
  return `hi ${who}`;
}

const double = (x: number) => x * 2;   // arrow function

if (age >= 18) console.log("adult");   // braces, semicolons optional
for (const x of items) console.log(x);

class Counter {
  private n = 0;                 // real access modifier (compile-time)
  inc() { this.n++; }
}
```

Key ideas:

- `string | number` means "one of these". Narrow with `if (typeof x === "string")`.
- `any` opts out of checking — avoid it; prefer `unknown` for untrusted data.
- `interface`/`type` describe shapes only; no runtime cost.
- Async is promises: `const data = await fetch(url).then(r => r.json())`.
- Errors: `try { ... } catch (e) { ... }`, plus `null`/`undefined` are distinct values.

See [hello.ts](hello.ts).

Official docs: [TypeScript Handbook](https://www.typescriptlang.org/docs/handbook/intro.html)
