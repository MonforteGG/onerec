mod paint;
mod save_dialog;
mod shell;
mod theme;

use ::windows::core::{w, HSTRING};
use ::windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use ::windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use ::windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

use crate::recorder::Recorder;
use crate::staging::StagingArea;
use crate::RunError;

pub(crate) fn run() -> Result<(), RunError> {
    let outcome = open();
    if let Err(error) = &outcome {
        report(error);
    }
    outcome
}

fn open() -> Result<(), RunError> {
    let _apartment = Apartment::enter()?;
    let staging = StagingArea::open().map_err(|error| {
        RunError::new(format!(
            "onerec could not prepare its temporary folder: {error}"
        ))
    })?;
    shell::run(Recorder::new(staging))
}

fn report(error: &RunError) {
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(error.to_string()),
            w!("onerec"),
            MB_OK | MB_ICONERROR,
        )
    };
}

/// The UI thread runs in an STA because `IFileSaveDialog` wants one.
struct Apartment {
    owned: bool,
}

impl Apartment {
    fn enter() -> Result<Self, RunError> {
        let result =
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        if result.is_ok() {
            Ok(Self { owned: true })
        } else if result == RPC_E_CHANGED_MODE {
            Ok(Self { owned: false })
        } else {
            Err(RunError::new(format!(
                "COM would not start on the UI thread: {}",
                result.message()
            )))
        }
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.owned {
            unsafe { CoUninitialize() };
        }
    }
}
