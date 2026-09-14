use std::io::{Read, Write};
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| s == "--help" || s == "-h") {
        println!(
            "Usage: oa [INPUT.json|-]\nRead a versioned analysis request and write JSON results to stdout.\nOmit INPUT or use - to read stdin. Errors go to stderr; exit status is 1.\nAnalysis types: static, modal, spectrum. All JSON quantities use SI units."
        );
        return Ok(());
    }
    if args.len() > 1 {
        return Err("expected at most one input file; use --help".into());
    }
    let input = if let Some(path) = args.first().filter(|s| s.as_str() != "-") {
        std::fs::read_to_string(path)?
    } else {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    };
    let result = oa_core::solve_json(&input)?;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    writeln!(lock, "{result}")?;
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{}", serde_json::json!({"error":error.to_string()}));
        std::process::exit(1);
    }
}
