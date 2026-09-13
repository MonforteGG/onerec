use std::fmt::Write;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::mp3::ExportQuality;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Prefs {
    pub microphone: Option<String>,
    pub output: Option<String>,
    pub quality: ExportQuality,
    pub folder: Option<PathBuf>,
    pub shortcut: Shortcut,
    pub last_file_name: Option<String>,
}

/// Win32 hot-key control encoding: virtual key in the low byte, modifier flags
/// (Shift, Ctrl, Alt, extended key) in the high byte. Zero disables registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Shortcut(pub u16);

impl Default for Shortcut {
    fn default() -> Self {
        Self(0x0500 | u16::from(b'R')) // Alt + Shift + R
    }
}

impl Shortcut {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        if value.is_empty() {
            return Some(Self(0));
        }
        let raw = value.parse::<u16>().ok()?;
        let key = raw & 0xff;
        if raw == 0
            || (raw & 0xf000 == 0 && key > 0 && ![0x10, 0x11, 0x12, 0x5b, 0x5c].contains(&key))
        {
            Some(Self(raw))
        } else {
            None
        }
    }
}

impl Prefs {
    #[cfg(test)]
    pub(crate) fn read(path: &Path) -> Self {
        fs::read_to_string(path)
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub(crate) fn parse(text: &str) -> Self {
        let mut prefs = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if key.trim() == "last_file_name" {
                if valid_file_name(value) {
                    prefs.last_file_name = Some(value.to_owned());
                }
                continue;
            }
            let value = value.trim();
            if key.trim() == "shortcut" {
                if let Some(shortcut) = Shortcut::parse(value) {
                    prefs.shortcut = shortcut;
                }
                continue;
            }
            if value.is_empty() {
                continue;
            }
            match key.trim() {
                "microphone" => prefs.microphone = Some(value.to_owned()),
                "output" => prefs.output = Some(value.to_owned()),
                "quality" => {
                    if let Some(quality) = ExportQuality::from_short_name(value) {
                        prefs.quality = quality;
                    }
                }
                "folder" => prefs.folder = Some(PathBuf::from(value)),
                _ => {}
            }
        }
        prefs
    }

    pub(crate) fn render(&self) -> String {
        let mut text = String::new();
        if let Some(id) = &self.microphone {
            let _ = writeln!(&mut text, "microphone={id}");
        }
        if let Some(id) = &self.output {
            let _ = writeln!(&mut text, "output={id}");
        }
        let _ = writeln!(&mut text, "quality={}", self.quality.short_name());
        if let Some(folder) = &self.folder {
            let _ = writeln!(&mut text, "folder={}", folder.display());
        }
        if self.shortcut.0 == 0 {
            let _ = writeln!(&mut text, "shortcut=");
        } else {
            let _ = writeln!(&mut text, "shortcut={}", self.shortcut.0);
        }
        if let Some(name) = &self.last_file_name {
            let _ = writeln!(&mut text, "last_file_name={name}");
        }
        text
    }

    pub(crate) fn write(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(dir)?;
            }
        }
        let tmp = path.with_extension("ini.tmp");
        fs::write(&tmp, self.render())?;
        fs::rename(&tmp, path)
    }
}

fn valid_file_name(value: &str) -> bool {
    !value.is_empty()
        && value.encode_utf16().count() <= 255
        && !value.ends_with('.')
        && !value.ends_with(' ')
        && !value
            .chars()
            .any(|c| c.is_control() || ['<', '>', ':', '"', '/', '\\', '|', '?', '*'].contains(&c))
}

#[cfg(windows)]
pub(crate) fn prefs_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("onerec.ini")))
        .unwrap_or_else(|| PathBuf::from("onerec.ini"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_defaults_and_explicit_disable_survive_round_trip() {
        assert_eq!(Prefs::default().shortcut, Shortcut(0x0552));
        assert_eq!(
            Prefs::parse("quality=Voice\n").shortcut,
            Shortcut::default()
        );
        for (value, expected) in [
            ("", Shortcut(0)),
            ("0", Shortcut(0)),
            ("838", Shortcut(838)),
        ] {
            let prefs = Prefs::parse(&format!("shortcut={value}\n"));
            assert_eq!(prefs.shortcut, expected);
            assert_eq!(Prefs::parse(&prefs.render()).shortcut, expected);
        }
        for invalid in ["nope", "65536", "65535", "1280", "16", "17", "18"] {
            assert_eq!(
                Prefs::parse(&format!("shortcut={invalid}")).shortcut,
                Shortcut::default()
            );
        }
    }

    #[test]
    fn last_file_name_is_exact() {
        for name in ["Reunión del equipo.mp3", "  Meeting.mp3", "name=part 2.mp3"] {
            let prefs = Prefs {
                last_file_name: Some(name.into()),
                ..Prefs::default()
            };
            assert_eq!(
                Prefs::parse(&prefs.render()).last_file_name.as_deref(),
                Some(name)
            );
        }
        for invalid in ["../escape.mp3", "C:\\escape.mp3", "bad:name.mp3", "", ".."] {
            assert_eq!(
                Prefs::parse(&format!("last_file_name={invalid}\n")).last_file_name,
                None
            );
        }
    }

    #[test]
    fn parse_round_trips_known_keys() {
        let text =
            "microphone={0.0.1}.mic\noutput={0.0.0}.out\nquality=Voice\nfolder=/tmp/meetings\n";
        let prefs = Prefs::parse(text);
        assert_eq!(prefs.microphone.as_deref(), Some("{0.0.1}.mic"));
        assert_eq!(prefs.output.as_deref(), Some("{0.0.0}.out"));
        assert_eq!(prefs.quality, ExportQuality::Voice);
        assert_eq!(prefs.folder.as_deref(), Some(Path::new("/tmp/meetings")));
        let again = Prefs::parse(&prefs.render());
        assert_eq!(again.quality, ExportQuality::Voice);
        assert_eq!(again.microphone, prefs.microphone);
    }

    #[test]
    fn unknown_quality_keeps_meeting() {
        assert_eq!(
            Prefs::parse("quality=Loud\n").quality,
            ExportQuality::Meeting
        );
    }

    #[test]
    fn write_then_read_restores_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("onerec.ini");
        let prefs = Prefs {
            microphone: Some("mic-b".into()),
            output: Some("out-b".into()),
            quality: ExportQuality::High,
            folder: Some(dir.path().to_path_buf()),
            shortcut: Shortcut::default(),
            last_file_name: Some("Reunión del equipo.mp3".into()),
        };
        prefs.write(&path).unwrap();
        let loaded = Prefs::read(&path);
        assert_eq!(loaded, prefs);
    }
}
