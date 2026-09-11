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

/// A windows-subsystem binary has no console, so the Windows build shows the failure in a
/// message box before `run()` returns it.
#[cfg(windows)]
fn report(_: &onerec::RunError) {}

#[cfg(not(windows))]
fn report(error: &onerec::RunError) {
    eprintln!("{error}");
}
