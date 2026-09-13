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

The V1 is a portable desktop app for Windows 10 and later, written in Rust with a native Win32 interface. Linux and macOS are out of scope. Audio capture, mixing, and MP3 encoding happen locally on your computer.

## Features

- **Microphone + system audio.** Capture both sources together using Windows WASAPI loopback, without a meeting app integration.
- **One window, one recording.** Device selectors, a prominent timer, separate audio meters, and clear recording and saving states. The meters move as soon as you pick a microphone and output device, so you can check the sources before pressing Record. The full interface stays visible during recording, including both audio meters and Settings.
- **Configurable global shortcut.** Start and stop with `Alt+Shift+R` by default, including when the app is in the background. Change the combination in Settings or clear it to disable the shortcut. Pause and Resume have a separate button.
- **Five MP3 profiles.** Choose small files for speech or higher bitrates for fuller audio.
- **Bring-your-own-key transcription and notes.** After a successful save, **Transcribe** sends the MP3 to an OpenAI-compatible API and writes a `{stem}.md` file next to the recording. **Notes** writes `{stem}.notes.md` with Summary, Decisions, Action items, and Open questions. If the markdown transcript is missing, Notes transcribes first, then summarizes, in one job. URL and model names start empty. Set them in Settings. You can edit the notes prompt, or leave it blank to use the built-in default.
- **Remembered preferences.** Restore your devices, quality, last successfully saved filename and folder, keyboard shortcut, nested-folder option, notes prompt, and API endpoint settings.
- **Responsive export.** Save with progress feedback while MP3 encoding runs in a background worker.
- **Native Windows presentation.** System fonts, per-monitor DPI scaling, high-contrast support, and a multi-resolution app icon.

## Getting started

Download `onerec.exe` from [GitHub Releases for this repository](https://github.com/MonforteGG/onerec/releases). Copy it to a folder you can write to. Double-click `onerec.exe`. No installer is required.

The shipped app is that single `onerec.exe`. LAME, the app icon, and the Lucide font are inside the executable. The first run creates `onerec.ini` beside the exe. If you redistribute the app, also ship `LICENSE` and `NOTICE`. Those files cover the MIT license and the LGPL LAME notice.

To build from source instead, see [Build from source](#build-from-source).

1. Select your **microphone** and the **output device** playing the meeting, such as your headphones or speakers. Speak and play audio: the meters should move if the right devices are selected.
2. Choose an **MP3 quality** before recording. On the first launch, **Meeting** is selected.
3. Click **Record** or press your shortcut (`Alt+Shift+R` by default). Both microphone and system-audio meters remain visible while recording.
4. Click **Pause** to freeze the timer and skip writing the take. Click **Resume** to continue on the same take.
5. Click **Stop** or press the shortcut again. The shortcut never pauses, and the window keeps the same layout.
6. Click **Save**. The save dialog always opens with the last successfully saved filename and folder. Edit the name or location as needed, then confirm and wait for export to finish. If **Save take in its own folder** is on, the MP3 is written to `folder/stem/stem.mp3`. The next Save dialog still opens in `folder`.
7. Click **Open folder** to find your MP3.
8. Click **Transcribe** to send the saved MP3 to your API. onerec writes a `.md` file next to the MP3 when it succeeds.
9. Click **Notes** to write a `{stem}.notes.md` file. If `{stem}.md` is missing, onerec transcribes first in the same job.

Before the first successful save, the suggested name is `Recording.mp3`. Later saves reuse your last filename exactly, without automatically adding dates or changing the name. Cancelling or a failed export does not update the remembered filename. If you keep an existing filename and nested folders are off, Windows asks before replacing the file. If nested folders are on, onerec asks before replacing `folder/stem/stem.mp3`. The interface uses English labels; device names come from Windows.

### After stopping

Stopping leaves the recording in **Awaiting save**; it does not open the save dialog automatically. Quality is locked for the current take, so choose it before pressing Start.

If you cancel the dialog or an export fails, the take stays available for another save attempt. Use **Save** to retry or **Discard** to delete it after confirmation. Save or discard the current take before starting another recording.

### Settings and keyboard shortcut

Open the gear at the top right, also available during recording. Settings has three tabs: Recording, API, and Notes. **Clear**, **Cancel**, and **Save** stay under the tabs.

On **Recording**, focus the shortcut field and press a combination, then click **Save**. The change applies immediately and is remembered next time. **Clear** followed by **Save** disables the global shortcut. **Cancel** keeps the previous settings. Unavailable combinations are rejected with an inline message.

The shortcut is temporarily suspended while Settings is open so you can enter it without starting or stopping a take. An ongoing recording continues normally.

On **API**, paste an OpenAI-compatible API key. Fill **Base URL**, **Transcribe model**, and **Notes model**. The fields start empty. There is no built-in provider. Groq is one example of an OpenAI-compatible API you can point at. `http://` URLs work for a local server. The key is stored in Windows Credential Manager, not in `onerec.ini`. Saving Settings with an empty key deletes the stored credential.

Turn on **Save take in its own folder** to write each take to `folder/stem/stem.mp3`. Sidecar markdown files land in that take folder. The remembered folder stays the save-dialog parent.

If you click Transcribe or Notes with a missing key, URL, or model, the idle status shows a short gap such as `No API key`, and Settings opens.

On **Notes**, edit the prompt used to write `{stem}.notes.md`. Leave it blank to use the built-in default.

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

Preferences are stored in `onerec.ini` beside the executable. The API key is stored in Windows Credential Manager. Keep the app in a writable folder so it can remember your choices.

Capture and mixing run at 48 kHz stereo. During recording, onerec writes a temporary take to the Windows temporary directory as 32-bit floating-point PCM, adapted to the selected quality:

| Profile | Temporary audio | Approx. temporary disk space per hour |
| --- | --- | --- |
| Meeting | 8 kHz mono | 115 MB |
| Voice | 16 kHz mono | 230 MB |
| Compact, Standard, High | 48 kHz stereo | 1.4 GB |

MP3 compression happens when you save. The temporary take is removed after a successful save or discard, and unsaved takes are not restored on the next launch. Since the temporary audio already reflects your selected profile, a Meeting take cannot later be exported as High.

The interface has no periodic refresh timer while awaiting save, or while minimized except during MP3 export or a sidecar job. While visible and idle or recording, meters update at up to 20 Hz. Minimizing releases idle device monitoring and pauses meter redraws; recording itself continues. Buffered PCM writes and MP3 export in bounded chunks keep disk and memory use controlled; export, transcription, and notes run on temporary worker threads.

## Build from source

Windows with the MSVC toolchain is the only supported build path for v1. You need:

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
- **The shortcut is unavailable:** another app or Windows may reserve that combination. Choose another one in Settings, or clear it and use Record / Stop instead.
- **A device disconnects:** if one source drops out during recording, onerec reports it and continues recording the remaining source.
- **A new recording cannot start:** save or discard the take that is awaiting save.
- **Transcribe or Notes shows `No API key` or `No API URL`:** open Settings. Paste a key and an OpenAI-compatible base URL. Groq is one example.
- **Transcribe or Notes shows `No transcribe model` or `No notes model`:** fill the matching model field on the API tab.
- **Transcription or notes fail:** check the key, model names, and that the MP3 is under 25 MB.
- **Preferences are not remembered:** check that the folder containing `onerec.exe` is writable.
- **Windows reports that `VCRUNTIME140.dll` is missing:** install the [Visual C++ 2015-2022 x64 redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist).

## License

onerec is licensed under the [MIT License](LICENSE). The bundled LAME encoder is licensed under the LGPL; see [NOTICE](NOTICE) for the third-party notice.
