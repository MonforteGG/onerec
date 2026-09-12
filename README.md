<p align="center">
  <img src="assets/onerec-256.png" alt="onerec logo" width="128" height="128">
</p>

<h1 align="center">onerec</h1>

<p align="center">
  Your microphone. Your meeting. One MP3.
</p>

<p align="center">
  Portable Windows app · Native interface · Local recording
</p>

onerec records your microphone and the audio playing through a selected output device into a single MP3. Built for meetings, calls, and voice recordings, it keeps the workflow simple: choose your devices, record, and save.

The V1 is a portable desktop app for Windows 10 and later, written in Rust with a native Win32 interface. Audio capture, mixing, and MP3 encoding happen locally on your computer.

## Features

- **Microphone + system audio.** Capture both sources together using Windows WASAPI loopback, without a meeting app integration.
- **One window, one recording.** Device selectors, a prominent timer, separate audio meters, and clear recording and saving states. While a take is live the window shrinks to a HUD with the timer, Stop, Pause or Resume, and the shortcut.
- **Global keyboard shortcut.** Start and stop with `Ctrl+Shift+R`, including when the app is in the background. Pause and Resume live on a separate button so the shortcut always ends the take.
- **Five MP3 profiles.** Choose small files for speech or higher bitrates for fuller audio.
- **Remembered preferences.** Restore your microphone, output device, quality, last save folder, and whether Save recording writes straight to that folder.
- **Responsive export.** Save with progress feedback while MP3 encoding runs in a background worker.
- **Native Windows presentation.** System fonts, per-monitor DPI scaling, high-contrast support, and a multi-resolution app icon.

## Getting started

Run `onerec.exe` from a folder you can write to. No installer is required. To create the executable from source, see [Build from source](#build-from-source).

1. Select your **microphone** and the **output device** playing the meeting, such as your headphones or speakers.
2. Choose an **MP3 quality** before recording. On the first launch, **Meeting** is selected.
3. Click **Start recording** or press `Ctrl+Shift+R`. The window shrinks to a HUD with the timer, Stop, Pause, and the shortcut.
4. Click **Pause** to freeze the timer and skip writing the take. Click **Resume** to continue on the same take.
5. Click **Stop recording** or press `Ctrl+Shift+R` again. The shortcut never pauses. The window grows back for Save.
6. Click **Save recording…**, choose a location, and wait for export to finish. While Idle, **Settings** can turn on Save directly to the last folder. Later saves then skip the dialog. **Save as…** still opens it.
7. Click **Open folder** to find your MP3.

The save dialog suggests a dated filename such as `2026-09-12 14-03 Meeting.mp3` and opens in your last save folder. The interface uses English labels; device names come from Windows.

### After stopping

Stopping leaves the recording in **Awaiting save**; it does not open the save dialog automatically. Quality is locked for the current take, so choose it before pressing Start.

If you cancel the dialog or an export fails, the take stays available for another save attempt. Use **Save recording…** to retry or **Discard…** to delete it after confirmation. Save or discard the current take before starting another recording.

## MP3 quality

| Profile | Bitrate | Sample rate | Channels | Approx. MP3 size per hour |
| --- | --- | --- | --- | --- |
| Meeting · default | 8 kbps | 8 kHz | Mono | 4 MB |
| Voice | 24 kbps | 16 kHz | Mono | 11 MB |
| Compact | 128 kbps | 48 kHz | Stereo | 56 MB |
| Standard | 192 kbps | 48 kHz | Stereo | 84 MB |
| High | 320 kbps | 48 kHz | Stereo | 141 MB |

Size estimates match the rounded values shown in the app. Stereo profiles use joint stereo encoding.

**Meeting** prioritizes small speech recordings. **Voice** retains more speech detail. **Compact**, **Standard**, and **High** preserve stereo audio at progressively higher bitrates.

## Local files and performance

Preferences are stored in `onerec.ini` beside the executable. Keep the app in a writable folder so it can remember your choices.

Capture and mixing run at 48 kHz stereo. During recording, onerec writes a temporary take to the Windows temporary directory as 32-bit floating-point PCM, adapted to the selected quality:

| Profile | Temporary audio | Approx. temporary disk space per hour |
| --- | --- | --- |
| Meeting | 8 kHz mono | 115 MB |
| Voice | 16 kHz mono | 230 MB |
| Compact, Standard, High | 48 kHz stereo | 1.4 GB |

MP3 compression happens when you save. The temporary take is removed after a successful save or discard, and unsaved takes are not restored on the next launch. Since the temporary audio already reflects your selected profile, a Meeting take cannot later be exported as High.

The interface has no periodic refresh timer while idle or awaiting save. During recording, meters update at up to 20 Hz, dropping to 2 Hz when minimized. Buffered PCM writes and MP3 export in bounded chunks keep disk and memory use controlled; export runs on a temporary worker thread.

## Build from source

The supported build path is Windows with the MSVC toolchain. You need:

- Rust and Cargo with the Windows MSVC toolchain.
- Visual Studio Build Tools with the C++ build tools and Windows SDK.
- The Windows SDK resource compiler, `rc.exe`, to embed the app icon.

From the repository root:

```powershell
cargo build --release --locked
.\target\release\onerec.exe
```

The executable is written to `target/release/onerec.exe`. LAME 3.100 is statically linked for MP3 encoding.

The build script looks for `rc.exe` on `PATH` and in standard Windows SDK locations. For a custom installation, set `ONEREC_RC` to the full path of `rc.exe` before building.

### Development checks

Run the test suite:

```powershell
cargo test --locked
```

For a hardware capture check on Windows:

```powershell
cargo run --locked --example probe
```

The probe lists audio devices, records two seconds from the default microphone and output device, and reports capture and mixed PCM statistics.

## Troubleshooting

- **Meeting audio is silent:** select the same output device your meeting app is using. Loopback captures audio playing through that device, including other apps using it.
- **The shortcut is unavailable:** another app may already own `Ctrl+Shift+R`. Use the recording button; onerec displays a message when registration fails.
- **A device disconnects:** if one source drops out during recording, onerec reports it and continues recording the remaining source.
- **A new recording cannot start:** save or discard the take that is awaiting save.
- **Preferences are not remembered:** check that the folder containing `onerec.exe` is writable.

## License

onerec is licensed under the [MIT License](LICENSE). The bundled LAME encoder is licensed under the LGPL; see [NOTICE](NOTICE) for the third-party notice.
