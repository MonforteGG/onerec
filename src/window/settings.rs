use ::windows::core::w;
use ::windows::Win32::Foundation::{BOOL, HWND};
use ::windows::Win32::UI::Controls::{
    TaskDialogIndirect, TDCBF_CANCEL_BUTTON, TDCBF_OK_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION,
    TDF_POSITION_RELATIVE_TO_WINDOW, TDF_SIZE_TO_CONTENT, TDF_VERIFICATION_FLAG_CHECKED,
    TASKDIALOGCONFIG, TASKDIALOG_COMMON_BUTTON_FLAGS, TASKDIALOG_FLAGS,
};
use ::windows::Win32::UI::WindowsAndMessaging::IDOK;

/// Nested Task Dialog. Uses the system modal loop, not a second app pump.
pub(crate) fn ask(owner: HWND, save_direct: bool) -> Option<bool> {
    let mut flags = TDF_ALLOW_DIALOG_CANCELLATION.0
        | TDF_SIZE_TO_CONTENT.0
        | TDF_POSITION_RELATIVE_TO_WINDOW.0;
    if save_direct {
        flags |= TDF_VERIFICATION_FLAG_CHECKED.0;
    }
    let config = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: owner,
        dwFlags: TASKDIALOG_FLAGS(flags),
        dwCommonButtons: TASKDIALOG_COMMON_BUTTON_FLAGS(
            TDCBF_OK_BUTTON.0 | TDCBF_CANCEL_BUTTON.0,
        ),
        pszWindowTitle: w!("Settings"),
        pszMainInstruction: w!("Settings"),
        pszContent: w!("When this is on and a folder is remembered, Save recording writes there with a dated name and skips the Save As dialog."),
        pszVerificationText: w!("Save directly to the last folder"),
        nDefaultButton: IDOK.0,
        ..Default::default()
    };
    let mut button = 0i32;
    let mut checked = BOOL::default();
    unsafe {
        TaskDialogIndirect(&config, Some(&mut button), None, Some(&mut checked)).ok()?;
    }
    if button != IDOK.0 {
        return None;
    }
    Some(checked.as_bool())
}
