mod cli;

fn main() {
    if let Err(e) = cli::cli(std::env::args().skip(1).collect()) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
