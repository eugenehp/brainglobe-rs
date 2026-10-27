//! The `brainglobe` command line.

use brainglobe_atlasapi::cli::{Parsed, parse_args, run};

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&argv) {
        Parsed::Help(text) => print!("{text}"),
        Parsed::Error(text, code) => {
            eprint!("{text}");
            std::process::exit(code);
        }
        Parsed::Run(args) => {
            if let Err(e) = run(&args) {
                // Python ends with a traceback; its last line is this
                eprintln!("{}: {e}", e.py_type());
                std::process::exit(1);
            }
        }
    }
}
