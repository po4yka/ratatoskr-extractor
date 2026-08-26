#![forbid(unsafe_code)]

//! Rewrites one explicitly named golden corpus expectation.

use std::io::Write as _;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut arguments = std::env::args_os();
    let program = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| "corpus-bless".to_owned());
    let Some(case) = arguments.next() else {
        eprintln!("usage: {program} <case>");
        return ExitCode::from(2);
    };
    if arguments.next().is_some() {
        eprintln!("usage: {program} <case>");
        return ExitCode::from(2);
    }
    let Ok(case) = case.into_string() else {
        eprintln!("case name must be valid UTF-8");
        return ExitCode::from(2);
    };
    match extractor_corpus::bless_case_at(extractor_corpus::corpus_root(), &case) {
        Ok(path) => {
            if writeln!(std::io::stdout().lock(), "blessed {}", path.display()).is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("failed to bless {case}: {error}");
            ExitCode::FAILURE
        }
    }
}
