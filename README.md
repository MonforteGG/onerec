# onerec

onerec records a Windows meeting into one mixed MP3.

## How to record a meeting

1. Run `onerec.exe`.
2. Pick a microphone and the output device that plays the meeting.
3. Pick an **MP3 quality**. Meeting is selected at launch (8 kbps, 8 kHz mono, about 4 MB per hour). Voice is 24 kbps. Compact, Standard, and High stay 128 kbps, 192 kbps, and 320 kbps at 48 kHz stereo.
4. Select **Start recording**, or press **Ctrl+Shift+R**.
5. Watch the meter under each device and the elapsed time.
6. Select **Stop recording**, or press **Ctrl+Shift+R** again.
7. Choose where to save the take. A Saving bar appears under MP3 quality. The status line shows the preset and a percent until the file is committed.

If you cancel the save dialog, onerec keeps the recording and stays in Awaiting save.
Choose **Save…** or **Discard**. You cannot start another recording until you do.

Staging is always 48 kHz stereo PCM. Meeting and Voice downsample and downmix when the MP3 is written.

v1 is a portable Windows 10 executable. This commit has WASAPI capture, the recorder window, and statically linked LAME 3.100.

Run `cargo test`. On Windows, `cargo run --example probe` lists devices and records two seconds of mixed PCM.
