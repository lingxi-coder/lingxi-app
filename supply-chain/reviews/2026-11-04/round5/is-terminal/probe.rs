use is_terminal_polyfill::IsTerminal as Polyfill;
use std::io::IsTerminal as Standard;

fn compare<T: Polyfill + Standard>(name: &str, value: &T, expected: bool) {
    let polyfill = Polyfill::is_terminal(value);
    let standard = Standard::is_terminal(value);
    assert_eq!(polyfill, standard, "{name} differs from std");
    assert_eq!(polyfill, expected, "unexpected terminal state for {name}");
    println!("{name}: polyfill={polyfill} std={standard}");
}

fn main() {
    let mut args = std::env::args().skip(1);
    let expected = match args.next().as_deref() {
        Some("tty") => true,
        Some("pipe") => false,
        _ => panic!("expected tty or pipe"),
    };
    let file = std::fs::File::create(args.next().expect("temporary file path")).unwrap();
    compare("regular-file", &file, false);
    let input = std::io::stdin();
    let output = std::io::stdout();
    let error = std::io::stderr();
    compare("stdin", &input, expected);
    compare("stdin-lock", &input.lock(), expected);
    compare("stdout", &output, expected);
    compare("stdout-lock", &output.lock(), expected);
    compare("stderr", &error, expected);
    compare("stderr-lock", &error.lock(), expected);
}
