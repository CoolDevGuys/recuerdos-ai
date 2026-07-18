mod bootstrap;
mod consolidation;
mod identity;
mod memories;
mod providers;
mod shared;
mod understanding;

fn main() {
    println!("recordagent {}", env!("CARGO_PKG_VERSION"));
}
