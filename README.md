# onerec

onerec records a Windows meeting into one mixed stereo MP3.

## How to record a meeting

1. Run `onerec.exe`.
2. Pick a microphone and the output device that plays the meeting.
3. Select **Start recording**, or press **Ctrl+Shift+R**.
4. Watch the meters and the elapsed time.
5. Select **Stop recording**, or press **Ctrl+Shift+R** again.
6. Choose where to save the take. The file is a 48 kHz stereo MP3.

If you cancel the save dialog, onerec keeps the recording and stays in Awaiting save.
Choose **Save...** or **Discard**. You cannot start another recording until you do.

v1 is a portable Windows 10 executable. This commit has WASAPI capture, the recorder window, and statically linked LAME 3.100.

Run `cargo test`. On Windows, `cargo run --example probe` lists devices and records two seconds of mixed PCM.
