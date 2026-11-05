fn greet(who: &str) -> String {
    format!("hi {who}")
}

fn main() {
    println!("{}", greet("world"));
    let squares: Vec<i32> = (0..5).map(|x| x * x).collect();
    println!("{squares:?}");
}
