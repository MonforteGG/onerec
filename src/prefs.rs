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
    pub transcribe_url: Option<String>,
    pub transcribe_model: Option<String>,
    pub notes_model: Option<String>,
    pub nest: bool,
    pub notes_prompt: Option<String>,
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

    pub(crate) fn legacy_api_key(text: &str) -> Option<String> {
        let mut found = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if key.trim() == "api_key" {
                let value = value.trim();
                found = (!value.is_empty()).then(|| value.to_owned());
            }
        }
        found
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
            if key.trim() == "api_key" {
                continue;
            }
            if key.trim() == "transcribe_url" {
                prefs.transcribe_url = (!value.is_empty()).then(|| value.to_owned());
                continue;
            }
            if key.trim() == "transcribe_model" {
                prefs.transcribe_model = (!value.is_empty()).then(|| value.to_owned());
                continue;
            }
            if key.trim() == "notes_model" || key.trim() == "chat_model" {
                prefs.notes_model = (!value.is_empty()).then(|| value.to_owned());
                continue;
            }
            if key.trim() == "nest" {
                prefs.nest = value == "1";
                continue;
            }
            if key.trim() == "notes_prompt" {
                let prompt = unescape_ini_value(value);
                prefs.notes_prompt = optional_text(&prompt);
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
        if let Some(url) = &self.transcribe_url {
            let _ = writeln!(&mut text, "transcribe_url={url}");
        }
        if let Some(model) = &self.transcribe_model {
            let _ = writeln!(&mut text, "transcribe_model={model}");
        }
        if let Some(model) = &self.notes_model {
            let _ = writeln!(&mut text, "notes_model={model}");
        }
        let _ = writeln!(&mut text, "nest={}", if self.nest { "1" } else { "0" });
        if let Some(prompt) = &self.notes_prompt {
            let _ = writeln!(&mut text, "notes_prompt={}", escape_ini_value(prompt));
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

pub(crate) fn optional_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

pub(crate) fn optional_url(value: &str) -> Option<String> {
    optional_text(value)
        .map(|url| url.trim_end_matches('/').to_owned())
        .filter(|url| !url.is_empty())
}

fn escape_ini_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

fn unescape_ini_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
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
        let leftover = "api_key=gsk_live\ntranscribe_url=https://api.openai.com/v1\ntranscribe_model=whisper-1\nchat_model=llama-3.1-8b-instant\n";
        assert_eq!(Prefs::legacy_api_key(leftover).as_deref(), Some("gsk_live"));
        let keyed = Prefs::parse(leftover);
        assert_eq!(
            keyed.transcribe_url.as_deref(),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(keyed.transcribe_model.as_deref(), Some("whisper-1"));
        assert_eq!(keyed.notes_model.as_deref(), Some("llama-3.1-8b-instant"));
        assert!(!keyed.render().contains("api_key="));
        assert!(!Prefs::parse("api_key=gsk_live\n").render().contains("api_key="));
        assert_eq!(Prefs::legacy_api_key("api_key=\n"), None);
        assert_eq!(Prefs::legacy_api_key("# api_key=secret\nquality=Voice\n"), None);
    }

    #[test]
    fn chat_model_reads_as_notes_model_and_never_writes_chat_model() {
        let from_old = Prefs::parse("chat_model=llama-3.1-8b-instant\n");
        assert_eq!(from_old.notes_model.as_deref(), Some("llama-3.1-8b-instant"));
        let rendered = from_old.render();
        assert!(rendered.contains("notes_model=llama-3.1-8b-instant"));
        assert!(!rendered.contains("chat_model="));
        let both = Prefs::parse("chat_model=old\nnotes_model=new\n");
        assert_eq!(both.notes_model.as_deref(), Some("new"));
        let last_wins = Prefs::parse("notes_model=new\nchat_model=old\n");
        assert_eq!(last_wins.notes_model.as_deref(), Some("old"));
    }

    #[test]
    fn nest_rewrites_as_zero_or_one() {
        assert!(!Prefs::parse("quality=Voice\n").nest);
        assert!(Prefs::parse("nest=1\n").nest);
        assert!(!Prefs::parse("nest=0\n").nest);
        assert!(!Prefs::parse("nest=true\n").nest);
        let on = Prefs {
            nest: true,
            ..Prefs::default()
        };
        assert!(on.render().lines().any(|line| line == "nest=1"));
        assert!(Prefs::default().render().lines().any(|line| line == "nest=0"));
    }

    #[test]
    fn notes_prompt_escapes_newlines_round_trip() {
        let prefs = Prefs {
            notes_prompt: Some("line1\nline2\tend\\slash".into()),
            ..Prefs::default()
        };
        let text = prefs.render();
        assert!(text
            .lines()
            .any(|line| line == "notes_prompt=line1\\nline2\\tend\\\\slash"));
        assert_eq!(
            Prefs::parse(&text).notes_prompt.as_deref(),
            Some("line1\nline2\tend\\slash")
        );
        assert_eq!(Prefs::parse("notes_prompt=\n").notes_prompt, None);
        assert_eq!(
            unescape_ini_value(r"keep\xunknown"),
            r"keep\xunknown"
        );
    }

    #[test]
    fn render_never_emits_api_key() {
        let prefs = Prefs {
            transcribe_url: Some("https://api.openai.com/v1".into()),
            transcribe_model: Some("whisper-1".into()),
            notes_model: Some("llama-3.1-8b-instant".into()),
            ..Prefs::default()
        };
        let text = prefs.render();
        assert!(!text.contains("api_key="));
        assert!(!text.contains("gsk_"));
        assert!(text.contains("notes_model=llama-3.1-8b-instant"));
        assert!(!text.contains("chat_model="));
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
            transcribe_url: Some("https://api.groq.com/openai/v1".into()),
            transcribe_model: Some("whisper-large-v3-turbo".into()),
            notes_model: Some("llama-3.1-8b-instant".into()),
            nest: true,
            notes_prompt: Some("hello\nworld".into()),
        };
        prefs.write(&path).unwrap();
        let loaded = Prefs::read(&path);
        assert_eq!(loaded, prefs);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("notes_model=llama-3.1-8b-instant"));
        assert!(!text.contains("chat_model="));
        assert!(text.lines().any(|line| line == "nest=1"));
        assert!(text.lines().any(|line| line == "notes_prompt=hello\\nworld"));
    }

    #[test]
    fn optional_url_trims_trailing_slashes() {
        assert_eq!(
            optional_url(" https://api.example.com/v1/ "),
            Some("https://api.example.com/v1".into())
        );
        assert_eq!(optional_url("   "), None);
        assert_eq!(optional_text("  whisper-1  "), Some("whisper-1".into()));
        assert_eq!(optional_text("\n"), None);
    }
}
