# Rust Basics

Compiled, statically typed systems language. Native speed like C/C++, but the compiler ("borrow checker") prevents memory bugs and data races. No garbage collector.

## Run it

```bash
rustc --version
cargo new app && cd app   # cargo = build tool + package manager
cargo run                 # build + run
cargo build --release     # optimized binary in target/release/
cargo test                # run tests
```

## Core syntax

```rust
fn main() {
    let name = "Ada";          // immutable by default
    let mut age = 36;          // mut = mutable
    age += 1;
    let items: Vec<i32> = vec![1, 2, 3];

    if age >= 18 {             // no parens needed; expression, not statement
        println!("adult");
    }
    for x in &items {
        println!("{x}");
    }
    let doubled: Vec<i32> = items.iter().map(|x| x * 2).collect();
    println!("{doubled:?}");
}

fn greet(who: &str) -> String {   // last expression = return value
    format!("hi {who}")
}

struct Counter { n: u32 }         // struct = data
impl Counter {                    // impl = methods
    fn inc(&mut self) { self.n += 1; }
}
```

Key ideas — the three that make Rust different:

1. **Ownership:** each value has one owner; when it goes out of scope it is freed. Moving a value invalidates the old binding.
2. **Borrowing:** `&x` lends read-only access, `&mut x` lends exclusive write access. Many readers XOR one writer — enforced at compile time.
3. **No null:** use `Option<T>` (`Some(v)` / `None`) and `Result<T, E>` (`Ok` / `Err`), handled with `match` or `?`.

```rust
fn divide(a: f64, b: f64) -> Result<f64, String> {
    if b == 0.0 { Err("divide by zero".into()) } else { Ok(a / b) }
}
```

See [hello.rs](hello.rs): `rustc hello.rs && ./hello`.

> **From the official docs:** *The Rust Programming Language* (The Book) presents ownership as “Rust’s most unique feature” — a set of compiler-checked rules that let Rust make memory-safety guarantees **without needing a garbage collector**. Borrowing (`&` / `&mut`) and the `Option`/`Result` handling above follow Book ch. 4 and ch. 9.

## Sources

- [The Rust Programming Language (The Book)](https://doc.rust-lang.org/book/) — ch. 4 [Ownership](https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html) and ch. 9 (error handling with `Result`).
