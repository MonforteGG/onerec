use std::path::{Path, PathBuf};

use crate::prefs::{optional_text, optional_url, Prefs};
use crate::vault::SecretStore;

const MAX_UPLOAD_BYTES: u64 = 25 * 1024 * 1024;
const BOUNDARY: &str = "----onerecTranscription";
const NOTES_HEADINGS: [&str; 4] = [
    "## Summary",
    "## Decisions",
    "## Action items",
    "## Open questions",
];
const NOTES_SYSTEM_PROMPT: &str = "\
You turn a meeting transcript into notes. Reply in the same language as the transcript.
Use exactly these markdown headings, in this order, and no title heading:
## Summary
## Decisions
## Action items
## Open questions
Under Action items use a markdown checklist. If a section has nothing, write 'None.'\
";

pub(crate) struct ReadyEndpoint {
    base_url: String,
    transcribe_model: String,
    notes_model: String,
}

impl std::fmt::Debug for ReadyEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadyEndpoint")
            .field("base_url", &self.base_url)
            .field("transcribe_model", &self.transcribe_model)
            .field("notes_model", &self.notes_model)
            .finish()
    }
}

pub(crate) struct Provider {
    api_key: String,
    endpoint: ReadyEndpoint,
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl Provider {
    fn snapshot(api_key: String, endpoint: ReadyEndpoint) -> Self {
        Self { api_key, endpoint }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JobKind {
    Transcript,
    Notes,
}

impl JobKind {
    pub(crate) fn progress_message(self) -> &'static str {
        match self {
            Self::Transcript => "Transcribing…",
            Self::Notes => "Writing notes…",
        }
    }

    pub(crate) fn success_prefix(self) -> &'static str {
        match self {
            Self::Transcript => "Transcribed",
            Self::Notes => "Notes",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidecarKind {
    Transcript,
    Notes,
}

#[derive(Debug)]
pub(crate) struct Job {
    pub kind: JobKind,
    pub audio: PathBuf,
    pub provider: Provider,
    pub notes_prompt: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IdleGap {
    Key,
    Url,
    TranscribeModel,
    NotesModel,
}

impl IdleGap {
    pub(crate) fn status(self) -> &'static str {
        match self {
            Self::Key => "No API key",
            Self::Url => "No API URL",
            Self::TranscribeModel => "No transcribe model",
            Self::NotesModel => "No notes model",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StartError {
    Vault(String),
    Gap(IdleGap),
}

pub(crate) fn prepare_job(
    vault: &dyn SecretStore,
    prefs: &Prefs,
    kind: JobKind,
    audio: &Path,
) -> Result<Job, StartError> {
    let api_key = match vault.load() {
        Err(detail) => return Err(StartError::Vault(detail)),
        Ok(None) => return Err(StartError::Gap(IdleGap::Key)),
        Ok(Some(key)) if key.trim().is_empty() => return Err(StartError::Gap(IdleGap::Key)),
        Ok(Some(key)) => key,
    };
    if let Some(gap) = gap_for(kind, prefs, audio) {
        return Err(StartError::Gap(gap));
    }
    let Some(base_url) = ready_url(prefs) else {
        return Err(StartError::Gap(IdleGap::Url));
    };
    let transcribe_model =
        optional_text(prefs.transcribe_model.as_deref().unwrap_or("")).unwrap_or_default();
    let notes_model = optional_text(prefs.notes_model.as_deref().unwrap_or("")).unwrap_or_default();
    Ok(Job {
        kind,
        audio: audio.to_path_buf(),
        provider: Provider::snapshot(
            api_key,
            ReadyEndpoint {
                base_url,
                transcribe_model,
                notes_model,
            },
        ),
        notes_prompt: resolved_notes_prompt(prefs.notes_prompt.as_deref()),
    })
}

fn gap_for(kind: JobKind, prefs: &Prefs, audio: &Path) -> Option<IdleGap> {
    match ready_url(prefs) {
        None => return Some(IdleGap::Url),
        Some(_) => {}
    }
    let transcribe = optional_text(prefs.transcribe_model.as_deref().unwrap_or(""));
    let notes = optional_text(prefs.notes_model.as_deref().unwrap_or(""));
    match kind {
        JobKind::Transcript => {
            if transcribe.is_none() {
                return Some(IdleGap::TranscribeModel);
            }
        }
        JobKind::Notes => {
            if !file_nonempty(&path(audio, SidecarKind::Transcript)) && transcribe.is_none() {
                return Some(IdleGap::TranscribeModel);
            }
            if notes.is_none() {
                return Some(IdleGap::NotesModel);
            }
        }
    }
    None
}

fn ready_url(prefs: &Prefs) -> Option<String> {
    let url = optional_url(prefs.transcribe_url.as_deref().unwrap_or(""))?;
    api_base_error(&url).is_none().then_some(url)
}

fn api_base_error(url: &str) -> Option<&'static str> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Some("The API URL must start with http:// or https://.");
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Some("The API URL cannot contain spaces.");
    }
    if url.contains(['?', '#']) {
        return Some("The API URL cannot contain a query or fragment.");
    }
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or("");
    if rest.is_empty() || rest.starts_with('/') {
        return Some("The API URL must include a host.");
    }
    None
}

pub(crate) fn validate_fields(
    base_url: &str,
    transcribe_model: &str,
    notes_model: &str,
) -> Result<(), String> {
    let url = base_url.trim().trim_end_matches('/');
    if !url.is_empty() {
        if let Some(detail) = api_base_error(url) {
            return Err(detail.into());
        }
    }
    for (label, model) in [
        ("transcribe", transcribe_model.trim()),
        ("notes", notes_model.trim()),
    ] {
        if model.is_empty() {
            continue;
        }
        if model
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || ['-', '_', '.', '/', ':'].contains(&c)))
        {
            return Err(format!("The {label} model name looks invalid."));
        }
    }
    Ok(())
}

pub(crate) fn resolved_notes_prompt(stored: Option<&str>) -> String {
    match stored {
        Some(text) if !text.trim().is_empty() => text.to_owned(),
        _ => NOTES_SYSTEM_PROMPT.to_owned(),
    }
}

pub(crate) fn path(audio: &Path, kind: SidecarKind) -> PathBuf {
    match kind {
        SidecarKind::Transcript => audio.with_extension("md"),
        SidecarKind::Notes => {
            let stem = audio
                .file_stem()
                .map(|stem| stem.to_string_lossy())
                .filter(|stem| !stem.is_empty())
                .unwrap_or_else(|| "Recording".into());
            audio.with_file_name(format!("{stem}.notes.md"))
        }
    }
}

pub(crate) fn run(job: &Job) -> Result<PathBuf, String> {
    match job.kind {
        JobKind::Transcript => run_transcript(&job.audio, &job.provider),
        JobKind::Notes => run_notes(job),
    }
}

fn title(audio: &Path) -> String {
    audio
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "Recording".into())
}

fn transcript_markdown(title: &str, body: &str) -> String {
    format!("# {title}\n\n{}\n", body.trim())
}

fn notes_markdown(model_text: &str) -> String {
    let trimmed = model_text.trim();
    if NOTES_HEADINGS
        .iter()
        .all(|heading| trimmed.contains(heading))
    {
        let mut body = trimmed.to_owned();
        if !body.ends_with('\n') {
            body.push('\n');
        }
        return body;
    }
    format!(
        "## Summary\n\n{trimmed}\n\n## Decisions\n\nNone.\n\n## Action items\n\nNone.\n\n## Open questions\n\nNone.\n"
    )
}

fn file_nonempty(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .is_some_and(|text| !text.trim().is_empty())
}

fn run_transcript(audio: &Path, provider: &Provider) -> Result<PathBuf, String> {
    let meta = std::fs::metadata(audio).map_err(|error| error.to_string())?;
    if meta.len() > MAX_UPLOAD_BYTES {
        return Err(format!(
            "This MP3 is larger than {} MB, the usual limit for OpenAI-compatible transcription.",
            MAX_UPLOAD_BYTES / (1024 * 1024)
        ));
    }
    let bytes = std::fs::read(audio).map_err(|error| error.to_string())?;
    let file_name = audio
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("recording.mp3");
    let body = multipart_body(file_name, &bytes, &provider.endpoint.transcribe_model);
    let url = format!(
        "{}/audio/transcriptions",
        provider.endpoint.base_url.trim_end_matches('/')
    );
    let response = post_multipart(&url, &provider.api_key, &body)?;
    let text = parse_response(response.status, &response.body)?;
    let dest = path(audio, SidecarKind::Transcript);
    std::fs::write(&dest, transcript_markdown(&title(audio), &text))
        .map_err(|error| error.to_string())?;
    Ok(dest)
}

fn run_notes(job: &Job) -> Result<PathBuf, String> {
    let transcript_path = path(&job.audio, SidecarKind::Transcript);
    let transcript = if file_nonempty(&transcript_path) {
        std::fs::read_to_string(&transcript_path).map_err(|error| error.to_string())?
    } else {
        run_transcript(&job.audio, &job.provider)?;
        std::fs::read_to_string(&transcript_path).map_err(|error| error.to_string())?
    };
    let url = format!(
        "{}/chat/completions",
        job.provider.endpoint.base_url.trim_end_matches('/')
    );
    let body = chat_body(
        &job.provider.endpoint.notes_model,
        &transcript,
        &job.notes_prompt,
    );
    let response = post_json(&url, &job.provider.api_key, &body)?;
    let content = parse_chat_response(response.status, &response.body)?;
    let dest = path(&job.audio, SidecarKind::Notes);
    std::fs::write(&dest, notes_markdown(&content)).map_err(|error| error.to_string())?;
    Ok(dest)
}

fn chat_body(model: &str, transcript: &str, prompt: &str) -> String {
    format!(
        "{{\"model\":{},\"messages\":[{{\"role\":\"system\",\"content\":{}}},{{\"role\":\"user\",\"content\":{}}}]}}",
        json_string(model),
        json_string(prompt),
        json_string(transcript),
    )
}

fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn multipart_body(file_name: &str, bytes: &[u8], model: &str) -> Vec<u8> {
    let safe_name = file_name.replace(['"', '\r', '\n', '\\'], "_");
    let mut body = Vec::with_capacity(bytes.len() + 512);
    write_text_part(&mut body, "model", model);
    write_text_part(&mut body, "response_format", "json");
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{safe_name}\"\r\n\
             Content-Type: audio/mpeg\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn write_text_part(body: &mut Vec<u8>, name: &str, value: &str) {
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes(),
    );
}

struct HttpResponse {
    status: u16,
    body: String,
}

fn post_multipart(url: &str, api_key: &str, body: &[u8]) -> Result<HttpResponse, String> {
    let response = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .post(url)
        .set("Authorization", &format!("Bearer {api_key}"))
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .send_bytes(body)
        .map_err(format_http_error)?;
    let status = response.status();
    let body = response.into_string().map_err(|error| error.to_string())?;
    Ok(HttpResponse { status, body })
}

fn post_json(url: &str, api_key: &str, body: &str) -> Result<HttpResponse, String> {
    let response = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .post(url)
        .set("Authorization", &format!("Bearer {api_key}"))
        .set("Content-Type", "application/json")
        .send_string(body)
        .map_err(format_http_error)?;
    let status = response.status();
    let body = response.into_string().map_err(|error| error.to_string())?;
    Ok(HttpResponse { status, body })
}

fn format_http_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(status, response) => {
            let body = response.into_string().unwrap_or_default();
            parse_response(status, &body)
                .err()
                .unwrap_or_else(|| format!("HTTP {status}"))
        }
        other => other.to_string(),
    }
}

fn parse_response(status: u16, body: &str) -> Result<String, String> {
    if (200..300).contains(&status) {
        if let Some(text) = json_string_pointer(body, "/text") {
            return Ok(text);
        }
        let trimmed = body.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_owned());
        }
        return Err("The transcription API returned an empty response.".into());
    }
    Err(api_error_text(body).unwrap_or_else(|| format!("Transcription failed (HTTP {status}).")))
}

fn parse_chat_response(status: u16, body: &str) -> Result<String, String> {
    if (200..300).contains(&status) {
        if let Some(text) = json_string_pointer(body, "/choices/0/message/content") {
            if !text.trim().is_empty() {
                return Ok(text);
            }
        }
        return Err("The chat API returned an empty response.".into());
    }
    Err(api_error_text(body).unwrap_or_else(|| format!("Notes failed (HTTP {status}).")))
}

fn json_string_pointer(body: &str, pointer: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value.pointer(pointer)?.as_str().map(str::to_owned)
}

fn api_error_text(body: &str) -> Option<String> {
    json_string_pointer(body, "/error/message")
        .or_else(|| json_string_pointer(body, "/message"))
        .or_else(|| json_string_pointer(body, "/error"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::MemoryVault;

    fn gap_status(
        vault: &dyn SecretStore,
        prefs: &Prefs,
        kind: JobKind,
        audio: &Path,
    ) -> &'static str {
        match prepare_job(vault, prefs, kind, audio) {
            Err(StartError::Gap(gap)) => gap.status(),
            other => panic!("expected a gap, got {other:?}"),
        }
    }

    fn endpoint_prefs() -> Prefs {
        Prefs {
            transcribe_url: Some("https://api.example.com/v1".into()),
            transcribe_model: Some("whisper-1".into()),
            notes_model: Some("notes-1".into()),
            ..Prefs::default()
        }
    }

    #[test]
    fn prepare_job_reports_each_idle_gap() {
        let audio = Path::new("take.mp3");
        let prefs = Prefs::default();
        assert_eq!(
            gap_status(&MemoryVault::default(), &prefs, JobKind::Transcript, audio),
            "No API key"
        );
        let vault = MemoryVault::from_key("gsk_test");
        assert_eq!(
            gap_status(&vault, &prefs, JobKind::Transcript, audio),
            "No API URL"
        );
        let mut prefs = Prefs {
            transcribe_url: Some("https://api.example.com/v1".into()),
            ..Prefs::default()
        };
        assert_eq!(
            gap_status(&vault, &prefs, JobKind::Transcript, audio),
            "No transcribe model"
        );
        prefs.transcribe_model = Some("whisper-1".into());
        let job = prepare_job(&vault, &prefs, JobKind::Transcript, audio).unwrap();
        assert_eq!(job.kind, JobKind::Transcript);
        assert_eq!(
            gap_status(&vault, &prefs, JobKind::Notes, audio),
            "No notes model"
        );
        prefs.notes_model = Some("notes-1".into());
        prefs.transcribe_model = None;
        assert_eq!(
            gap_status(&vault, &prefs, JobKind::Notes, audio),
            "No transcribe model"
        );
        assert_eq!(IdleGap::Key.status(), "No API key");
        assert_eq!(IdleGap::Url.status(), "No API URL");
        assert_eq!(IdleGap::TranscribeModel.status(), "No transcribe model");
        assert_eq!(IdleGap::NotesModel.status(), "No notes model");
    }

    #[test]
    fn garbage_url_is_no_api_url_at_job_start() {
        let vault = MemoryVault::from_key("gsk_test");
        let prefs = Prefs {
            transcribe_url: Some("not-a-url".into()),
            transcribe_model: Some("whisper-1".into()),
            notes_model: Some("notes-1".into()),
            ..Prefs::default()
        };
        assert_eq!(
            gap_status(&vault, &prefs, JobKind::Transcript, Path::new("take.mp3")),
            "No API URL"
        );
        let prefs = Prefs {
            transcribe_url: Some("https://api.groq.com/openai/v1?foo=1".into()),
            transcribe_model: Some("whisper-1".into()),
            notes_model: Some("notes-1".into()),
            ..Prefs::default()
        };
        assert_eq!(
            gap_status(&vault, &prefs, JobKind::Transcript, Path::new("take.mp3")),
            "No API URL"
        );
    }

    #[test]
    fn blank_prompt_uses_the_built_in_text_at_job_time() {
        let vault = MemoryVault::from_key("gsk_test");
        let mut prefs = endpoint_prefs();
        let job = prepare_job(&vault, &prefs, JobKind::Notes, Path::new("take.mp3")).unwrap();
        assert_eq!(job.notes_prompt, resolved_notes_prompt(None));
        assert!(job.notes_prompt.contains("## Summary"));
        assert!(job.notes_prompt.contains("## Decisions"));
        assert!(job.notes_prompt.contains("## Action items"));
        assert!(job.notes_prompt.contains("## Open questions"));
        prefs.notes_prompt = Some("Use bullets only.".into());
        let custom = prepare_job(&vault, &prefs, JobKind::Notes, Path::new("take.mp3")).unwrap();
        assert_eq!(custom.notes_prompt, "Use bullets only.");
        prefs.notes_prompt = Some("   ".into());
        let blank = prepare_job(&vault, &prefs, JobKind::Notes, Path::new("take.mp3")).unwrap();
        assert_eq!(blank.notes_prompt, resolved_notes_prompt(None));
    }

    #[test]
    fn notes_with_existing_transcript_does_not_need_a_transcribe_model() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("take.mp3");
        std::fs::write(&audio, b"mp3").unwrap();
        std::fs::write(audio.with_extension("md"), "# take\n\nhello\n").unwrap();
        let vault = MemoryVault::from_key("gsk_test");
        let prefs = Prefs {
            transcribe_url: Some("https://api.example.com/v1".into()),
            notes_model: Some("notes-1".into()),
            ..Prefs::default()
        };
        let job = prepare_job(&vault, &prefs, JobKind::Notes, &audio).unwrap();
        assert_eq!(job.kind, JobKind::Notes);
        assert!(prepare_job(&vault, &prefs, JobKind::Transcript, &audio).is_err());
    }

    #[test]
    fn validate_fields_allows_empty_url_and_models() {
        assert!(validate_fields("", "", "").is_ok());
        assert!(validate_fields("https://api.openai.com/v1", "whisper-1", "gpt-4o-mini").is_ok());
        assert!(validate_fields(
            "http://127.0.0.1:8080/v1",
            "whisper-1",
            "llama-3.3-70b-versatile"
        )
        .is_ok());
        assert!(validate_fields("https://api.groq.com/openai/v1", "", "").is_ok());
        assert!(validate_fields("not-a-url", "", "").is_err());
        assert!(validate_fields("https://ok", "bad model", "").is_err());
        assert!(validate_fields("https://api.groq.com/openai/v1?foo=1", "", "").is_err());
        assert!(validate_fields("https://api.groq.com/openai/v1#frag", "", "").is_err());
        assert_eq!(
            validate_fields("https://api.groq.com/openai/v1?foo=1", "", "").unwrap_err(),
            "The API URL cannot contain a query or fragment."
        );
    }

    #[test]
    fn multipart_includes_model_and_file_bytes() {
        let body = multipart_body("take.mp3", b"ID3DATA", "whisper-large-v3-turbo");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("name=\"model\""));
        assert!(text.contains("whisper-large-v3-turbo"));
        assert!(text.contains("filename=\"take.mp3\""));
        assert!(body.windows(7).any(|chunk| chunk == b"ID3DATA"));
    }

    #[test]
    fn parse_response_reads_json_text_and_api_errors() {
        assert_eq!(
            parse_response(200, r#"{"text":"Hola \"mundo\"\n"}"#).unwrap(),
            "Hola \"mundo\"\n"
        );
        assert_eq!(
            parse_response(200, "plain transcript").unwrap(),
            "plain transcript"
        );
        assert!(
            parse_response(401, r#"{"error":{"message":"Invalid API Key"}}"#)
                .unwrap_err()
                .contains("Invalid API Key")
        );
        assert_eq!(
            parse_chat_response(
                200,
                "{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"## Summary\\nHi\"}}]}",
            )
            .unwrap(),
            "## Summary\nHi"
        );
        assert_eq!(
            parse_response(
                200,
                r#"{"meta":{"text":"NOPE"},"text":"hello","segments":[{"text":" skip"}]}"#,
            )
            .unwrap(),
            "hello"
        );
        assert_eq!(
            parse_response(200, r#"{"text":"hi \uD83D\uDE00"}"#).unwrap(),
            "hi 😀"
        );
        assert_eq!(
            parse_chat_response(
                200,
                r#"{"id":"content","choices":[{"logprobs":{"content":[]},"message":{"content":"Hi"}}]}"#,
            )
            .unwrap(),
            "Hi"
        );
    }

    #[test]
    fn sidecar_paths_use_stem_notes_suffix() {
        assert_eq!(
            path(Path::new(r"C:\takes\Meeting.mp3"), SidecarKind::Transcript),
            PathBuf::from(r"C:\takes\Meeting.md")
        );
        assert_eq!(
            path(Path::new(r"C:\takes\Meeting.mp3"), SidecarKind::Notes),
            PathBuf::from(r"C:\takes\Meeting.notes.md")
        );
        assert_eq!(
            path(Path::new(r"C:\takes\foo.bar.mp3"), SidecarKind::Notes),
            PathBuf::from(r"C:\takes\foo.bar.notes.md")
        );
        assert_ne!(
            path(Path::new("Meeting.mp3"), SidecarKind::Notes),
            PathBuf::from("Meeting.md")
        );
    }

    #[test]
    fn markdown_shapes_transcript_and_wraps_notes() {
        assert_eq!(
            transcript_markdown("Meeting", "hello\n"),
            "# Meeting\n\nhello\n"
        );
        let wrapped = notes_markdown("We hired Ada.");
        assert!(wrapped.starts_with("## Summary\n\nWe hired Ada."));
        assert!(wrapped.contains("## Decisions\n\nNone."));
        assert!(wrapped.contains("## Action items\n\nNone."));
        assert!(wrapped.contains("## Open questions\n\nNone.\n"));
        let complete = "## Summary\nOk.\n## Decisions\nShip it.\n## Action items\n- [ ] Write tests\n## Open questions\nNone.";
        assert_eq!(notes_markdown(complete), format!("{complete}\n"));
    }

    #[test]
    fn provider_debug_omits_the_key() {
        let vault = MemoryVault::from_key("gsk_secret_value");
        let job = prepare_job(
            &vault,
            &endpoint_prefs(),
            JobKind::Transcript,
            Path::new("take.mp3"),
        )
        .unwrap();
        let debug = format!("{:?}", job.provider);
        assert!(!debug.contains("gsk_secret_value"));
        assert!(!debug.contains("api_key"));
    }

    #[test]
    fn chat_body_uses_the_job_prompt() {
        let body = chat_body("notes-1", "hello", "Use headings.");
        assert!(body.contains("\"Use headings.\""));
        assert!(!body.contains(NOTES_SYSTEM_PROMPT));
    }
}
