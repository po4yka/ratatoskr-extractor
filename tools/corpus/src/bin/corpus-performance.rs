#![forbid(unsafe_code)]

//! Emits an offline corpus performance report and optionally checks its baseline.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use extractor_corpus::performance::{PerformanceBaseline, check, measure};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("corpus performance failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut iterations = 100_usize;
    let mut max_rss_kib = 0_u64;
    let mut check_baseline = false;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--iterations" => {
                let value = arguments.next().ok_or("--iterations requires a value")?;
                iterations = value
                    .parse()
                    .map_err(|_| "--iterations must be an integer")?;
            }
            "--max-rss-kib" => {
                let value = arguments.next().ok_or("--max-rss-kib requires a value")?;
                max_rss_kib = value
                    .parse()
                    .map_err(|_| "--max-rss-kib must be an integer")?;
            }
            "--check" => check_baseline = true,
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    let report = measure(iterations, max_rss_kib).map_err(|error| error.to_string())?;
    if check_baseline {
        let baseline_path = PathBuf::from(extractor_corpus::corpus_root())
            .join("performance")
            .join("baseline.json");
        let source = std::fs::read(&baseline_path)
            .map_err(|error| format!("could not read {}: {error}", baseline_path.display()))?;
        let baseline = serde_json::from_slice::<PerformanceBaseline>(&source)
            .map_err(|error| format!("could not decode {}: {error}", baseline_path.display()))?;
        check(&report, &baseline).map_err(|error| error.to_string())?;
    }
    writeln!(
        std::io::stdout().lock(),
        "{}",
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
    )
    .map_err(|error| error.to_string())
}
