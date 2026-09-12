use std::fmt::Write;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::time::{SystemTime, UNIX_EPOCH};

use crate::mp3::ExportQuality;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Prefs {
    pub microphone: Option<String>,
    pub output: Option<String>,
    pub quality: ExportQuality,
    pub folder: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CivilTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
}

impl Prefs {
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
            let value = value.trim();
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

impl CivilTime {
    pub(crate) fn local() -> Self {
        #[cfg(windows)]
        {
            let now = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
            return Self {
                year: now.wYear,
                month: now.wMonth as u8,
                day: now.wDay as u8,
                hour: now.wHour as u8,
                minute: now.wMinute as u8,
            };
        }
        #[cfg(not(windows))]
        {
            utc_from_unix(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            )
        }
    }
}

pub(crate) fn dated_file_name(quality: ExportQuality, when: CivilTime) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}-{:02} {}",
        when.year,
        when.month,
        when.day,
        when.hour,
        when.minute,
        quality.short_name()
    )
}

#[cfg(windows)]
pub(crate) fn prefs_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("onerec.ini")))
        .unwrap_or_else(|| PathBuf::from("onerec.ini"))
}

#[cfg(not(windows))]
fn utc_from_unix(secs: i64) -> CivilTime {
    let days = secs.div_euclid(86400);
    let tod = secs.rem_euclid(86400) as u32;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
    let year = (y + i64::from(month <= 2)) as u16;
    CivilTime {
        year,
        month,
        day,
        hour: (tod / 3600) as u8,
        minute: ((tod % 3600) / 60) as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dated_name_matches_the_meeting_example() {
        assert_eq!(
            dated_file_name(
                ExportQuality::Meeting,
                CivilTime {
                    year: 2026,
                    month: 9,
                    day: 12,
                    hour: 14,
                    minute: 3,
                }
            ),
            "2026-09-12 14-03 Meeting"
        );
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
        };
        prefs.write(&path).unwrap();
        let loaded = Prefs::read(&path);
        assert_eq!(loaded, prefs);
    }
}
