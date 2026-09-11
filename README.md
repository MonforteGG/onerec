# onerec

onerec records a Windows meeting into one mixed MP3.

## How to record a meeting

1. Run `onerec.exe`.
2. Pick a microphone and the output device that plays the meeting.
3. Pick an **MP3 quality**. Meeting is selected at launch (8 kbps, 8 kHz mono, about 4 MB per hour). Voice is 24 kbps. Compact, Standard, and High stay 128 kbps, 192 kbps, and 320 kbps at 48 kHz stereo.
4. Select **Start recording**, or press **Ctrl+Shift+R**.
5. Watch the meter under each device and the elapsed time.
6. Select **Stop recording**, or press **Ctrl+Shift+R** again.
7. Select **Save recording…**, then choose the destination. A separate progress bar shows the export of the 48 kHz stereo MP3.
8. After saving, select **Open folder** to find the file.

If you cancel the save dialog, onerec keeps the recording and stays in Awaiting save.
Choose **Save recording…** to retry or **Discard…** to delete the take after confirmation. You cannot start another recording until you do. Stopping does not open the save dialog automatically.

Staging is always 48 kHz stereo PCM. Meeting and Voice downsample and downmix when the MP3 is written.

v1 is a portable Windows 10 executable. This commit has WASAPI capture, the recorder window, and statically linked LAME 3.100.

Run `cargo test`. On Windows, `cargo run --example probe` lists devices and records two seconds of mixed PCM.

## UI and app icon

The UI uses native Win32 controls, system fonts, a prominent timer, and one primary action for each recording state. Meters show input level in dBFS, silence, or clipping. The window supports Per-Monitor V2 DPI scaling and Windows high-contrast colors. The interface keeps its existing English labels; device names come from Windows.

There is no periodic UI timer while idle or awaiting save. Recording meters refresh at up to 20 Hz, reduced to 2 Hz when minimized. MP3 export runs in one temporary worker thread with bounded buffers, while the UI observes progress. Capture and mixing retain their existing worker and audio settings.

Windows MSVC builds embed the multi-resolution icon from `assets/onerec.ico` and use it for the window's large and small icons. Building requires the Windows SDK resource compiler (`rc.exe`); set `ONEREC_RC` if it is installed in a custom location.

```powershell
cargo build --release
./target/release/onerec.exe
```
