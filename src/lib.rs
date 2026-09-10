mod capture;
mod ids;
mod session;
mod timeline;

use std::fmt;

pub fn run() -> Result<(), RunError> {
    #[cfg(windows)]
    {
        Err(RunError {
            message: "Windows capture is not in this build".into(),
        })
    }
    #[cfg(not(windows))]
    {
        Err(RunError {
            message: "v1 is Windows-only".into(),
        })
    }
}

#[derive(Debug)]
pub struct RunError {
    message: String,
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RunError {}

#[cfg(test)]
mod tests {
    #[test]
    fn run_reports_windows_only_off_windows() {
        #[cfg(not(windows))]
        {
            assert_eq!(crate::run().unwrap_err().to_string(), "v1 is Windows-only");
        }
    }
}
