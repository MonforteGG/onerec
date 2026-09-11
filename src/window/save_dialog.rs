use std::ffi::c_void;
use std::path::PathBuf;

use ::windows::core::{HSTRING, PCWSTR};
use ::windows::Win32::Foundation::HWND;
use ::windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use ::windows::Win32::System::SystemInformation::GetLocalTime;
use ::windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use ::windows::Win32::UI::Shell::{
    FOLDERID_Documents, FileSaveDialog, IFileSaveDialog, IShellItem, SHCreateItemFromParsingName,
    SHGetKnownFolderPath, KF_FLAG_DEFAULT, SIGDN_FILESYSPATH,
};

use crate::recorder::SavePrompt;

/// A dismissed dialog and a broken one both answer `None`, because neither is a reason to
/// lose the take.
pub(crate) fn ask_destination(owner: HWND, prompt: &SavePrompt) -> Option<PathBuf> {
    let label = HSTRING::from(prompt.filter_label);
    let pattern = HSTRING::from(format!("*.{}", prompt.extension));
    let extension = HSTRING::from(prompt.extension);
    let suggestion = HSTRING::from(default_name(prompt));
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dialog
            .SetFileTypes(&[COMDLG_FILTERSPEC {
                pszName: PCWSTR(label.as_ptr()),
                pszSpec: PCWSTR(pattern.as_ptr()),
            }])
            .ok()?;
        dialog.SetDefaultExtension(&extension).ok()?;
        dialog.SetFileName(&suggestion).ok()?;
        // The default folder, not the folder: a second save reopens wherever the user went.
        if let Some(documents) = documents() {
            let _ = dialog.SetDefaultFolder(&documents);
        }
        dialog.Show(owner).ok()?;
        let item = dialog.GetResult().ok()?;
        let chosen = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = chosen.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(chosen.0 as *const c_void));
        path
    }
}

fn default_name(prompt: &SavePrompt) -> String {
    let now = unsafe { GetLocalTime() };
    format!(
        "{}-{:04}{:02}{:02}-{:02}{:02}{:02}.{}",
        prompt.name_stem,
        now.wYear,
        now.wMonth,
        now.wDay,
        now.wHour,
        now.wMinute,
        now.wSecond,
        prompt.extension
    )
}

unsafe fn documents() -> Option<IShellItem> {
    let path = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None).ok()?;
    let folder = SHCreateItemFromParsingName(PCWSTR(path.0 as *const u16), None).ok();
    CoTaskMemFree(Some(path.0 as *const c_void));
    folder
}
