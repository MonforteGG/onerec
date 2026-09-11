#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    match onerec::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            report(&error);
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn report(_: &onerec::RunError) {}

#[cfg(not(windows))]
fn report(error: &onerec::RunError) {
    eprintln!("{error}");
}
