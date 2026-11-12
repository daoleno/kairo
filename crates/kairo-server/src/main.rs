//! `kairo-server`: Kairo with its UI as a web page. See `kairo-server help`.
//! It runs where the Unix socket and signals it relies on exist.

#[cfg(unix)]
mod cli;

#[cfg(unix)]
fn main() {
    cli::main()
}

#[cfg(not(unix))]
fn main() {
    eprintln!("kairo-server runs on Linux and macOS.");
    std::process::exit(1);
}
