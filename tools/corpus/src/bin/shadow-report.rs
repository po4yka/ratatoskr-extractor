#![forbid(unsafe_code)]

//! Renders or verifies the offline legacy shadow-comparison report.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use extractor_corpus::shadow::{render_report, verify_report_at};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("shadow comparison failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut check = false;
    let mut output = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--check" => check = true,
            "--output" => {
                let value = arguments.next().ok_or("--output requires a path")?;
                output = Some(PathBuf::from(value));
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    if check {
        verify_report_at(extractor_corpus::corpus_root()).map_err(|error| error.to_string())?;
    }
    let report = render_report().map_err(|error| error.to_string())?;
    match output {
        Some(path) => std::fs::write(path, report).map_err(|error| error.to_string()),
        None => std::io::stdout()
            .lock()
            .write_all(report.as_bytes())
            .map_err(|error| error.to_string()),
    }
}
