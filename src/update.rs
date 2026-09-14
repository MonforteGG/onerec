use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const RELEASES_LATEST: &str = "https://api.github.com/repos/MonforteGG/onerec/releases/latest";
const USER_AGENT: &str = concat!("onerec/", env!("CARGO_PKG_VERSION"));
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_DOWNLOAD_BYTES: u64 = 80 * 1024 * 1024;
const PE_MAGIC: &[u8] = b"MZ";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}

impl Version {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let value = value.trim().trim_start_matches('v');
        let mut parts = value.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }

    pub(crate) fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION")
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Release {
    pub version: Version,
    pub download_url: String,
    pub zip: bool,
}

pub(crate) fn check() -> Result<Option<Release>, String> {
    let body = get_text(RELEASES_LATEST, CHECK_TIMEOUT)?;
    Ok(parse_latest_release(&body).filter(|release| release.version > Version::current()))
}

pub(crate) fn parse_latest_release(body: &str) -> Option<Release> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let version = Version::parse(value.get("tag_name")?.as_str()?)?;
    let assets = value.get("assets")?.as_array()?;
    let (download_url, zip) = asset_download(assets, "onerec.zip", true)
        .or_else(|| asset_download(assets, "onerec.exe", false))?;
    Some(Release {
        version,
        download_url,
        zip,
    })
}

fn asset_download(
    assets: &[serde_json::Value],
    file_name: &str,
    zip: bool,
) -> Option<(String, bool)> {
    assets.iter().find_map(|asset| {
        let name = asset.get("name")?.as_str()?;
        name.eq_ignore_ascii_case(file_name)
            .then(|| {
                asset
                    .get("browser_download_url")?
                    .as_str()
                    .map(|url| (url.to_owned(), zip))
            })
            .flatten()
    })
}

pub(crate) fn install(release: &Release) -> Result<(), String> {
    let current = current_exe()?;
    install_into(&current, release)
}

pub(crate) fn install_into(current: &Path, release: &Release) -> Result<(), String> {
    let downloaded = with_suffix(current, "new");
    let archive = with_suffix(current, "zip");
    let result = (|| {
        if release.zip {
            download_to(&release.download_url, &archive)?;
            extract_onerec_exe(&archive, &downloaded)?;
        } else {
            download_to(&release.download_url, &downloaded)?;
        }
        if !looks_like_windows_exe(&downloaded) {
            return Err("The download was not a Windows executable.".into());
        }
        replace_exe(current, &downloaded)
    })();
    let _ = fs::remove_file(&archive);
    if result.is_err() {
        let _ = fs::remove_file(&downloaded);
    }
    result
}

pub(crate) fn relaunch() -> Result<(), String> {
    let current = current_exe()?;
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    std::process::Command::new(&current)
        .args(args)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("The new onerec could not start: {error}"))
}

pub(crate) fn remove_stale_files() {
    if let Ok(current) = current_exe() {
        remove_stale(&current);
    }
}

pub(crate) fn remove_stale(current: &Path) {
    for suffix in ["old", "new", "zip"] {
        let _ = fs::remove_file(with_suffix(current, suffix));
    }
}

pub(crate) fn replace_exe(current: &Path, downloaded: &Path) -> Result<(), String> {
    let old = with_suffix(current, "old");
    let _ = fs::remove_file(&old);
    fs::rename(current, &old).map_err(|error| {
        format!("Could not replace onerec.exe. Check that the folder is writable. {error}")
    })?;
    if let Err(error) = fs::rename(downloaded, current) {
        let _ = fs::rename(&old, current);
        return Err(format!("Could not install the new onerec.exe: {error}"));
    }
    Ok(())
}

fn current_exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|error| format!("Could not locate onerec.exe: {error}"))
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    match path.file_name() {
        Some(name) => {
            let mut file = name.to_os_string();
            file.push(".");
            file.push(suffix);
            path.with_file_name(file)
        }
        None => path.with_extension(suffix),
    }
}

fn looks_like_windows_exe(path: &Path) -> bool {
    let mut magic = [0u8; 2];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .ok()
        .is_some_and(|_| magic == PE_MAGIC)
}

fn extract_onerec_exe(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|error| error.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    let index = (0..zip.len())
        .find(|&i| {
            zip.by_index(i)
                .ok()
                .is_some_and(|entry| !entry.is_dir() && zip_file_name(entry.name()) == "onerec.exe")
        })
        .ok_or_else(|| "The zip did not contain onerec.exe.".to_string())?;
    let mut entry = zip.by_index(index).map_err(|error| error.to_string())?;
    if entry.size() > MAX_DOWNLOAD_BYTES {
        return Err("The download was larger than expected.".into());
    }
    let mut out = File::create(dest).map_err(|error| error.to_string())?;
    io::copy(&mut entry, &mut out).map_err(|error| error.to_string())?;
    out.flush().map_err(|error| error.to_string())?;
    Ok(())
}

fn zip_file_name(name: &str) -> String {
    name.rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase()
}

fn get_text(url: &str, timeout: Duration) -> Result<String, String> {
    let response = agent(timeout)
        .get(url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(format_http_error)?;
    let status = response.status();
    let body = response.into_string().map_err(|error| error.to_string())?;
    if !(200..300).contains(&status) {
        return Err(format!("GitHub returned HTTP {status}"));
    }
    Ok(body)
}

fn download_to(url: &str, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
    }
    let part = dest.with_extension(format!("{}.part", std::process::id()));
    let result = (|| {
        let response = agent(DOWNLOAD_TIMEOUT)
            .get(url)
            .set("User-Agent", USER_AGENT)
            .call()
            .map_err(format_http_error)?;
        let status = response.status();
        if !(200..300).contains(&status) {
            return Err(format!("Download failed (HTTP {status})"));
        }
        let mut reader = response.into_reader().take(MAX_DOWNLOAD_BYTES + 1);
        let mut file = File::create(&part).map_err(|error| error.to_string())?;
        let copied = io::copy(&mut reader, &mut file).map_err(|error| error.to_string())?;
        file.flush().map_err(|error| error.to_string())?;
        if copied == 0 {
            return Err("The download was empty.".into());
        }
        if copied > MAX_DOWNLOAD_BYTES {
            return Err("The download was larger than expected.".into());
        }
        Ok(())
    })();
    match result {
        Ok(()) => fs::rename(&part, dest).map_err(|error| {
            let _ = fs::remove_file(&part);
            error.to_string()
        }),
        Err(error) => {
            let _ = fs::remove_file(&part);
            Err(error)
        }
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(timeout).build()
}

fn format_http_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(status, _) => format!("GitHub returned HTTP {status}"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parses_tags_and_orders_semver() {
        assert_eq!(
            Version::parse("v1.0.2"),
            Some(Version {
                major: 1,
                minor: 0,
                patch: 2
            })
        );
        assert_eq!(Version::parse("1.0.2"), Version::parse("v1.0.2"));
        assert!(Version::parse("1.0.3").unwrap() > Version::parse("1.0.2").unwrap());
        assert!(Version::parse("1.1.0").unwrap() > Version::parse("1.0.9").unwrap());
        assert!(Version::parse("2.0.0").unwrap() > Version::parse("1.9.9").unwrap());
        assert_eq!(Version::parse("1.0"), None);
        assert_eq!(Version::parse("1.0.2.1"), None);
        assert_eq!(Version::parse("nope"), None);
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn parse_latest_release_reads_the_onerec_asset() {
        let zip_body = r#"{
            "tag_name": "v9.8.7",
            "assets": [
                {"name": "LICENSE", "browser_download_url": "https://example/LICENSE"},
                {"name": "onerec.exe", "browser_download_url": "https://example/onerec.exe"},
                {"name": "onerec.zip", "browser_download_url": "https://example/onerec.zip"}
            ]
        }"#;
        let release = parse_latest_release(zip_body).unwrap();
        assert_eq!(release.version.to_string(), "9.8.7");
        assert_eq!(release.download_url, "https://example/onerec.zip");
        assert!(release.zip);
        let exe_only = r#"{
            "tag_name": "v9.8.7",
            "assets": [
                {"name": "onerec.exe", "browser_download_url": "https://example/onerec.exe"}
            ]
        }"#;
        let fallback = parse_latest_release(exe_only).unwrap();
        assert_eq!(fallback.download_url, "https://example/onerec.exe");
        assert!(!fallback.zip);
        assert!(parse_latest_release(r#"{"tag_name":"v1.0.0","assets":[]}"#).is_none());
        assert!(parse_latest_release("not json").is_none());
    }

    fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in files {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn extract_onerec_exe_ignores_other_files_and_folders() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("onerec.zip");
        let dest = dir.path().join("onerec.exe.new");
        write_zip(
            &archive,
            &[
                ("LICENSE", b"mit"),
                ("folder/onerec.exe", b"MZ-from-folder"),
            ],
        );
        extract_onerec_exe(&archive, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"MZ-from-folder");
        write_zip(&archive, &[("NOTICE", b"lgpl")]);
        assert!(extract_onerec_exe(&archive, &dest).is_err());
    }

    #[test]
    fn replace_exe_swaps_the_running_name_and_keeps_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("onerec.exe");
        let downloaded = dir.path().join("onerec.exe.new");
        fs::write(&current, b"MZ-old").unwrap();
        fs::write(&downloaded, b"MZ-new").unwrap();
        replace_exe(&current, &downloaded).unwrap();
        assert_eq!(fs::read(&current).unwrap(), b"MZ-new");
        assert_eq!(fs::read(with_suffix(&current, "old")).unwrap(), b"MZ-old");
        assert!(!downloaded.exists());
        remove_stale(&current);
        assert!(!with_suffix(&current, "old").exists());
        assert!(!with_suffix(&current, "new").exists());
    }

    #[test]
    fn looks_like_windows_exe_requires_mz_header() {
        let dir = tempfile::tempdir().unwrap();
        let pe = dir.path().join("onerec.exe");
        let other = dir.path().join("notes.md");
        fs::write(&pe, b"MZ\x90\x00").unwrap();
        fs::write(&other, b"<html>").unwrap();
        assert!(looks_like_windows_exe(&pe));
        assert!(!looks_like_windows_exe(&other));
    }

    #[test]
    fn replace_exe_restores_the_original_when_the_new_file_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("onerec.exe");
        fs::write(&current, b"MZ-keep").unwrap();
        let missing = dir.path().join("missing.exe");
        assert!(replace_exe(&current, &missing).is_err());
        assert_eq!(fs::read(&current).unwrap(), b"MZ-keep");
    }
}
