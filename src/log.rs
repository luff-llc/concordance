//! `tracker/log.jsonl`: an append-only log of facts (reported, fixed, verified, reopened,
//! uploaded, released, pulled) beside catalog.toml, and the `concordance log` commands that
//! append to it. `track` and `suite` derive issue and release states from it; see
//! docs/tracker.md ("Log") and docs/adopt.md.
use crate::tracker::{calendar_day, valid_issue_id, IssueKind};
use crate::{Catalog, Impact, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

pub const LOG_FILE: &str = "tracker/log.jsonl";
/// Longest slug `log report` derives from a title, before any `-2` suffix.
const SLUG_MAX: usize = 48;

/// `[tracker]` in catalog.toml.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackerConfig {
    /// Release tag globs with one `{version}` placeholder, e.g. `ios/v{version}`.
    #[serde(default)]
    pub tag_pattern: TagPatterns,
}

/// One pattern or several.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum TagPatterns {
    One(String),
    Many(Vec<String>),
}
impl Default for TagPatterns {
    fn default() -> Self {
        Self::Many(vec![])
    }
}
impl TagPatterns {
    pub fn patterns(&self) -> &[String] {
        match self {
            Self::One(one) => std::slice::from_ref(one),
            Self::Many(many) => many,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Reported,
    Fixed,
    Verified,
    Reopened,
    Uploaded,
    Released,
    Pulled,
}
impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reported => "reported",
            Self::Fixed => "fixed",
            Self::Verified => "verified",
            Self::Reopened => "reopened",
            Self::Uploaded => "uploaded",
            Self::Released => "released",
            Self::Pulled => "pulled",
        }
    }
    /// Events about an issue (the rest are about a release).
    pub fn is_issue(self) -> bool {
        matches!(
            self,
            Self::Reported | Self::Fixed | Self::Verified | Self::Reopened
        )
    }
}

/// One fact. Serialized with a stable key order; empty optional fields are omitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub id: String,
    pub kind: EventKind,
    /// Issue ID or release ID.
    pub subject: String,
    /// UTC, `YYYY-MM-DDTHH:MM:SSZ`.
    pub at: String,
    pub by: String,
    /// A stable source key set by automated sources (`git:trailer:…`, `git:tag:…`, or a
    /// tool's `--key`). An event whose key is already in the log is already recorded;
    /// events without a key are always appended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platforms: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<Impact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Older logs only: the fix commit a verification was bound to when recorded. Still
    /// read (a check bound to an earlier fix does not verify a newer one); never written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// Release date, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Older logs only; read and ignored, never written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<u32>,
}

impl Event {
    /// A new event with its id computed; kind-specific fields start empty.
    pub fn new(kind: EventKind, subject: &str, at: &str, by: &str) -> Self {
        Self {
            id: String::new(),
            kind,
            subject: subject.into(),
            at: at.into(),
            by: by.into(),
            key: None,
            title: None,
            entries: vec![],
            platforms: vec![],
            impact: None,
            source: vec![],
            commit: None,
            fix: None,
            evidence: vec![],
            platform: None,
            build: None,
            date: None,
            reason: None,
            cycle: None,
        }
    }
    /// Give the event a fresh unique id. Ids never repeat across appends, so two unkeyed
    /// facts with identical fields are two lines; a line duplicated byte for byte (a
    /// repeated write) shares its id and still loads once.
    pub fn with_id(mut self) -> Self {
        self.id = new_id(&self);
        self
    }

    /// The JSON line, without its newline.
    pub fn line(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    /// Kind-specific structure: required and forbidden fields, formats and the id.
    pub fn structural_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        let kind = self.kind.as_str();
        if !valid_timestamp(&self.at) {
            errors.push(format!(
                "at {:?} must be a UTC time YYYY-MM-DDTHH:MM:SSZ",
                self.at
            ));
        }
        if self.by.trim().is_empty() {
            errors.push("by must not be empty".into());
        }
        if self.kind.is_issue() {
            let ok = issue_kind(&self.subject).is_some_and(|k| valid_issue_id(&self.subject, k));
            if !ok {
                errors.push(format!(
                    "subject {:?} must be an issue id (bug. or request., then lowercase letters, numbers and hyphens)",
                    self.subject
                ));
            }
        } else if self.subject.trim().is_empty() || self.subject.chars().any(char::is_whitespace) {
            errors.push(format!(
                "subject {:?} must be a release id without spaces",
                self.subject
            ));
        }
        let present: [(&str, bool); 13] = [
            ("title", self.title.is_some()),
            ("entries", !self.entries.is_empty()),
            ("platforms", !self.platforms.is_empty()),
            ("impact", self.impact.is_some()),
            ("source", !self.source.is_empty()),
            ("commit", self.commit.is_some()),
            ("fix", self.fix.is_some()),
            ("evidence", !self.evidence.is_empty()),
            ("platform", self.platform.is_some()),
            ("build", self.build.is_some()),
            ("date", self.date.is_some()),
            ("reason", self.reason.is_some()),
            ("cycle", self.cycle.is_some()),
        ];
        let (required, optional): (&[&str], &[&str]) = match self.kind {
            EventKind::Reported => (
                &["title"],
                &["entries", "platforms", "impact", "source"][..],
            ),
            EventKind::Fixed => (&["commit"], &["cycle"]),
            EventKind::Verified => (&["evidence"], &["commit", "fix", "cycle"]),
            EventKind::Reopened => (&["reason"], &[]),
            EventKind::Uploaded => (&["platform", "build"], &["commit"]),
            EventKind::Released => (&["date"], &["platform", "commit", "cycle"]),
            EventKind::Pulled => (&["reason"], &[]),
            // `key` may be on any kind; it is checked below.
        };
        for (field, is_present) in present {
            if required.contains(&field) && !is_present {
                errors.push(format!("{kind} requires {field}"));
            } else if is_present && !required.contains(&field) && !optional.contains(&field) {
                errors.push(format!("{field} is not allowed on {kind}"));
            }
        }
        let blank = |v: &Option<String>| v.as_deref().is_some_and(|s| s.trim().is_empty());
        for (field, value) in [
            ("title", &self.title),
            ("commit", &self.commit),
            ("fix", &self.fix),
            ("platform", &self.platform),
            ("build", &self.build),
            ("reason", &self.reason),
            ("key", &self.key),
        ] {
            if blank(value) {
                errors.push(format!("{field} must not be empty"));
            }
        }
        if self.cycle == Some(0) {
            errors.push("cycle must be at least 1; omit it otherwise".into());
        }
        if self.evidence.iter().any(|e| e.trim().is_empty()) {
            errors.push("evidence items must not be empty".into());
        }
        if let Some(date) = &self.date {
            if calendar_day(date).is_none() {
                errors.push(format!("date {date:?} is not a calendar date (YYYY-MM-DD)"));
            }
        }
        let hex = self
            .id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
        if self.id.len() != 16 || !hex {
            errors.push(format!("id {:?} must be 16 lowercase hex digits", self.id));
        }
        errors
    }
}

/// 64 bits from /dev/urandom, mixed with the process id, the time, a per-process sequence
/// number and the event's content (all of it, if /dev/urandom is unavailable).
fn new_id(event: &Event) -> String {
    use std::io::Read as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let mut random = [0u8; 8];
    let _ = fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut random));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let content = serde_json::to_string(&Event {
        id: String::new(),
        ..event.clone()
    })
    .unwrap_or_default();
    let seed = format!(
        "{}\u{1f}{}\u{1f}{nanos}\u{1f}{}\u{1f}{content}",
        u64::from_le_bytes(random),
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    format!("{:016x}", fnv1a(seed.as_bytes()))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The issue kind named by an ID's prefix.
pub fn issue_kind(id: &str) -> Option<IssueKind> {
    if id.starts_with("bug.") {
        Some(IssueKind::Bug)
    } else if id.starts_with("request.") {
        Some(IssueKind::Request)
    } else {
        None
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` with a real date and time.
pub fn valid_timestamp(text: &str) -> bool {
    let b = text.as_bytes();
    if b.len() != 20 || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z' {
        return false;
    }
    let number = |range: std::ops::Range<usize>| -> Option<u32> {
        text.get(range)
            .filter(|p| p.bytes().all(|c| c.is_ascii_digit()))
            .and_then(|p| p.parse().ok())
    };
    text.get(0..10).and_then(calendar_day).is_some()
        && number(11..13).is_some_and(|h| h < 24)
        && number(14..16).is_some_and(|m| m < 60)
        && number(17..19).is_some_and(|s| s < 60)
}

/// Seconds since the epoch as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn timestamp_from_unix(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // Inverse of days_from_civil.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Now, in UTC, to the second.
pub fn now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    timestamp_from_unix(seconds)
}

/// A `--at` value: a full UTC timestamp, or a date meaning midnight UTC.
pub fn parse_at(text: &str) -> Result<String> {
    if valid_timestamp(text) {
        return Ok(text.into());
    }
    if calendar_day(text).is_some() {
        return Ok(format!("{text}T00:00:00Z"));
    }
    Err(format!("--at {text:?} must be YYYY-MM-DDTHH:MM:SSZ (UTC) or YYYY-MM-DD").into())
}

/// At most `n` characters of `text`, cut on a character boundary.
pub fn prefix(text: &str, n: usize) -> &str {
    match text.char_indices().nth(n) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

/// The `YYYY-MM-DD` day of a timestamp (validated timestamps are ASCII).
pub fn day(at: &str) -> String {
    prefix(at, 10).to_owned()
}

/// Lowercase ASCII letters and digits joined by single hyphens, at most `SLUG_MAX` bytes,
/// cut at a hyphen when possible.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out = out.trim_end_matches('-').to_owned();
    if out.len() > SLUG_MAX {
        let cut = out[..=SLUG_MAX]
            .rfind('-')
            .filter(|&i| i > SLUG_MAX / 2)
            .unwrap_or(SLUG_MAX);
        out.truncate(cut);
        out = out.trim_end_matches('-').to_owned();
    }
    if out.is_empty() {
        "untitled".into()
    } else {
        out
    }
}

/// An event and its 1-based line number.
pub type NumberedEvent = (usize, Event);
/// A line number and what is wrong with that line.
pub type LineError = (usize, String);

/// A parsed log: valid events, errors and warnings, each with its 1-based line number.
#[derive(Debug, Default)]
pub struct ParsedLog {
    pub events: Vec<NumberedEvent>,
    pub errors: Vec<LineError>,
    /// Duplicate ids whose content differs (the first line is kept), and a torn last line.
    pub warnings: Vec<LineError>,
    /// Byte offset where an unterminated, unparseable last line starts: a torn write,
    /// ignored on load and cut off by the next writer.
    pub torn_at: Option<usize>,
}

/// Parse log text strictly: every nonblank line must be one JSON event. Blank lines are
/// ignored (and never written). A repeated id keeps the first line; when the repeat's
/// content differs, it is a warning, never an error, so two writers racing to record the
/// same fact cannot make the log unloadable.
pub fn parse(text: &str) -> ParsedLog {
    parse_bytes(text.as_bytes())
}

/// `parse` over raw bytes, decoding each line on its own: a torn last line cut inside a
/// multibyte character is a torn write, and invalid UTF-8 elsewhere names its line.
pub fn parse_bytes(bytes: &[u8]) -> ParsedLog {
    let mut parsed = ParsedLog::default();
    let mut seen: std::collections::BTreeMap<String, (usize, Event)> =
        std::collections::BTreeMap::new();
    let mut keys: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
    let last = lines.len() - 1;
    let mut offset = 0;
    for (index, raw) in lines.iter().enumerate() {
        let number = index + 1;
        let start = offset;
        offset += raw.len() + 1;
        let torn = |parsed: &mut ParsedLog| {
            parsed.warnings.push((
                number,
                "unterminated last line does not parse (a torn write); ignored".into(),
            ));
            parsed.torn_at = Some(start);
        };
        let line = match std::str::from_utf8(raw) {
            Ok(line) => line,
            Err(_) if index == last => {
                torn(&mut parsed);
                continue;
            }
            Err(error) => {
                parsed
                    .errors
                    .push((number, format!("line is not valid UTF-8: {error}")));
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let event: Event = match serde_json::from_str(line) {
            Ok(event) => event,
            // Unterminated and unparseable: a writer died mid-line.
            Err(_) if index == last => {
                torn(&mut parsed);
                continue;
            }
            Err(error) => {
                parsed.errors.push((number, error.to_string()));
                continue;
            }
        };
        let problems = event.structural_errors();
        if !problems.is_empty() {
            parsed
                .errors
                .extend(problems.into_iter().map(|p| (number, p)));
            continue;
        }
        if let Some((first, kept)) = seen.get(&event.id) {
            if *kept != event {
                parsed.warnings.push((
                    number,
                    format!(
                        "event {} repeats line {first} with different content; line {first} is kept",
                        event.id
                    ),
                ));
            }
            continue;
        }
        // Replay counts a source key once: the first line in file order (two checkouts
        // that scanned the same tag, merged by git, give two lines).
        if let Some(key) = &event.key {
            if let Some(first) = keys.get(key) {
                parsed.warnings.push((
                    number,
                    format!("key {key} repeated on line {number} (first on line {first}); ignored"),
                ));
                continue;
            }
            keys.insert(key.clone(), number);
        }
        seen.insert(event.id.clone(), (number, event.clone()));
        parsed.events.push((number, event));
    }
    parsed
}

// ---------------------------------------------------------------------------------------
// The `concordance log` commands.

#[derive(clap::Args)]
pub struct LogArgs {
    /// Catalog directory (default: $CONCORDANCE_CATALOG, then the current directory if it
    /// holds catalog.toml, then the only spec/*/catalog.toml below it).
    #[arg(long, global = true)]
    pub catalog: Option<PathBuf>,
    /// Who records the fact (default: $CONCORDANCE_ACTOR, then git config user.name,
    /// then "unknown").
    #[arg(long, global = true)]
    pub by: Option<String>,
    /// When, UTC: YYYY-MM-DDTHH:MM:SSZ or YYYY-MM-DD (default: now).
    #[arg(long, global = true)]
    pub at: Option<String>,
    /// A stable source key for tools (for example `release-taupe:<upload id>`): a second
    /// event with the same key is already recorded. People leave it out. Not for scan-git,
    /// which sets its own.
    #[arg(long, global = true)]
    pub key: Option<String>,
    #[command(subcommand)]
    pub action: LogAction,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum ReportKind {
    Bug,
    Request,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum ImpactArg {
    Critical,
    High,
    Medium,
    Low,
}

#[derive(clap::Subcommand)]
pub enum LogAction {
    /// Report a bug or request; prints the new issue id (bug.<slug> or request.<slug>).
    Report {
        #[arg(value_enum)]
        kind: ReportKind,
        title: String,
        #[arg(long = "entry")]
        entries: Vec<String>,
        #[arg(long = "platform")]
        platforms: Vec<String>,
        #[arg(long, value_enum)]
        impact: Option<ImpactArg>,
        #[arg(long)]
        source: Vec<String>,
    },
    /// Record the commit that fixes an issue.
    Fixed {
        issue: String,
        #[arg(long)]
        commit: String,
    },
    /// Record a passing check; counts as verified only when --by differs from the fixer.
    Verified {
        issue: String,
        #[arg(long, required = true, num_args = 1..)]
        evidence: Vec<String>,
        #[arg(long)]
        commit: Option<String>,
    },
    /// Send an issue back to open.
    Reopened {
        issue: String,
        #[arg(long)]
        reason: String,
    },
    /// Record an uploaded build of a release on one platform.
    Uploaded {
        release: String,
        #[arg(long)]
        platform: String,
        #[arg(long)]
        build: String,
        #[arg(long)]
        commit: Option<String>,
    },
    /// Record that a release went out (default date: the day of --at).
    Released {
        release: String,
        #[arg(long)]
        platform: Option<String>,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        commit: Option<String>,
    },
    /// Record that a release was pulled.
    Pulled {
        release: String,
        #[arg(long)]
        reason: String,
    },
    /// Append facts stated by commit trailers (Fixes:, Verifies:, Reports:) and release
    /// tags matching [tracker] tag_pattern. Idempotent.
    ScanGit {
        /// Repository (default: the catalog's repo_root).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Only commits after this ref (REF..HEAD), and tags not contained in it.
        #[arg(long)]
        since: Option<String>,
    },
}

/// Find the catalog directory for `log`.
fn catalog_dir(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(dir) = explicit {
        return Ok(dir);
    }
    if let Some(dir) = std::env::var_os("CONCORDANCE_CATALOG").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    if Path::new("catalog.toml").is_file() {
        return Ok(PathBuf::from("."));
    }
    let mut found = Vec::new();
    if let Ok(entries) = fs::read_dir("spec") {
        for entry in entries.flatten() {
            if entry.path().join("catalog.toml").is_file() {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err("no catalog found; pass --catalog DIR or set CONCORDANCE_CATALOG".into()),
        _ => Err(format!(
            "several catalogs under spec/ ({}); pass --catalog DIR",
            found
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into()),
    }
}

/// $CONCORDANCE_ACTOR, then `git config user.name` in `repo`, then "unknown".
pub fn default_actor(repo: &Path) -> String {
    if let Some(actor) = std::env::var("CONCORDANCE_ACTOR")
        .ok()
        .filter(|a| !a.trim().is_empty())
    {
        return actor;
    }
    crate::scan_git::git(repo, &["config", "user.name"])
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// The repository a catalog's commits live in: `repo_root` relative to the catalog.
pub fn repo_dir(catalog: &Catalog) -> PathBuf {
    catalog.directory.join(&catalog.config.repo_root)
}

/// How long a writer waits for the log lock.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// The lock file beside the log. Writers hold an OS advisory lock on it (released by the
/// OS if the process dies); the file itself stays.
pub const LOCK_FILE: &str = "tracker/log.jsonl.lock";

/// A queued write: a finished event, or a report whose issue id is allocated under the
/// lock.
enum Pending {
    Event(Event),
    Report { kind: IssueKind, event: Event },
}

/// A loaded catalog plus its log. Events are validated and queued with `append` (or
/// `report`), then written together by `flush` under the lock.
pub struct LogWriter {
    pub catalog: Catalog,
    path: PathBuf,
    /// Source keys recorded or queued.
    keys: BTreeSet<String>,
    pending: Vec<Pending>,
}

/// Take the advisory lock on `path`, polling for up to `LOCK_WAIT`.
fn lock_log(path: &Path) -> Result<fs::File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let start = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(fs::TryLockError::WouldBlock) => {
                if start.elapsed() > LOCK_WAIT {
                    return Err(format!(
                        "another writer has held {} for {} s; try again",
                        path.display(),
                        LOCK_WAIT.as_secs()
                    )
                    .into());
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(fs::TryLockError::Error(error)) => {
                return Err(format!("{}: {error}", path.display()).into())
            }
        }
    }
}

/// Refuse to write through a symlinked `tracker/`, log or lock, or outside the catalog.
fn check_write_paths(directory: &Path) -> Result<()> {
    let tracker = directory.join("tracker");
    for path in [
        tracker.clone(),
        directory.join(LOG_FILE),
        directory.join(LOCK_FILE),
    ] {
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!("refusing to write through symlink {}", path.display()).into());
        }
    }
    if tracker.exists() {
        let root = fs::canonicalize(directory)?;
        let resolved = fs::canonicalize(&tracker)?;
        if !resolved.starts_with(&root) {
            return Err(format!(
                "refusing to write: {} resolves outside the catalog directory",
                tracker.display()
            )
            .into());
        }
    }
    Ok(())
}

impl LogWriter {
    /// Load the catalog; refuse when the existing log has errors.
    pub fn open(dir: &Path) -> Result<Self> {
        let (catalog, report) = Catalog::load_collect(dir)?;
        let log_errors: Vec<String> = report
            .errors
            .iter()
            .filter(|e| e.group == "log")
            .map(|e| format!("{}: {}", e.reference, e.message))
            .collect();
        if !log_errors.is_empty() {
            return Err(format!(
                "{LOG_FILE} has errors; fix them before appending:\n{}",
                log_errors.join("\n")
            )
            .into());
        }
        let keys = catalog
            .tracker
            .log
            .iter()
            .filter_map(|e| e.key.clone())
            .collect();
        Ok(Self {
            path: catalog.directory.join(LOG_FILE),
            catalog,
            keys,
            pending: vec![],
        })
    }

    /// Issue IDs known from issues.yaml or a `reported` event.
    pub fn known_issue(&self, id: &str) -> bool {
        self.catalog.tracker.issue(id).is_some()
    }

    fn check(&self, event: &Event) -> Result<()> {
        let mut problems = event.structural_errors();
        problems.extend(self.catalog.event_reference_errors(event));
        if problems.is_empty() {
            return Ok(());
        }
        Err(format!(
            "{} {}: {}",
            event.kind.as_str(),
            event.subject,
            problems.join("; ")
        )
        .into())
    }

    /// Validate and queue one event. Returns false when its source key is already
    /// recorded or queued; an event without a key is always queued. Nothing is written
    /// until `flush`.
    pub fn append(&mut self, event: &Event) -> Result<bool> {
        let event = event.clone().with_id();
        self.check(&event)?;
        if let Some(key) = &event.key {
            if !self.keys.insert(key.clone()) {
                return Ok(false);
            }
        }
        self.catalog.tracker.record(&event);
        self.pending.push(Pending::Event(event));
        Ok(true)
    }

    /// The recorded event carrying `event`'s source key (for "already recorded" output),
    /// or `event` itself when none is found.
    pub fn recorded_with_key(&self, event: &Event) -> Event {
        self.catalog
            .tracker
            .log
            .iter()
            .find(|e| e.key.is_some() && e.key == event.key)
            .cloned()
            .unwrap_or_else(|| event.clone())
    }

    /// Queue a report. Its subject is ignored: the issue id is allocated in `flush`,
    /// under the lock, after re-reading the log.
    pub fn report(&mut self, kind: IssueKind, event: Event) -> Result<()> {
        let errors = self.catalog.event_reference_errors(&event);
        if !errors.is_empty() {
            return Err(format!("reported: {}", errors.join("; ")).into());
        }
        self.pending.push(Pending::Report { kind, event });
        Ok(())
    }

    /// The id for a report: an issue of the same kind and title (in the catalog, the
    /// log as re-read, or this flush), else `<kind>.<slug>` made unique with -2, -3, ...
    fn allocate_report_id(
        &self,
        kind: IssueKind,
        title: &str,
        recent: &[Event],
        subjects: &BTreeSet<String>,
    ) -> String {
        let tracker = &self.catalog.tracker;
        let same = tracker
            .issues
            .iter()
            .find(|i| i.kind == kind && i.title == title)
            .map(|i| i.id.clone())
            .or_else(|| {
                recent
                    .iter()
                    .find(|e| {
                        e.kind == EventKind::Reported
                            && e.title.as_deref() == Some(title)
                            && issue_kind(&e.subject) == Some(kind)
                    })
                    .map(|e| e.subject.clone())
            });
        if let Some(id) = same {
            return id;
        }
        let base = format!("{}.{}", kind.as_str(), slug(title));
        let taken = |id: &str| {
            tracker.issue(id).is_some()
                || self.catalog.entries.contains_key(id)
                || subjects.contains(id)
        };
        if !taken(&base) {
            return base;
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|id| !taken(id))
            .unwrap_or(base)
    }

    /// Write every queued event in one append, under the lock. The log is re-read under
    /// the lock: a keyed event another writer recorded meanwhile is not written twice,
    /// report ids are allocated against it, and a torn last line is cut off first. Returns each
    /// queued event and whether it was written.
    pub fn flush(&mut self) -> Result<Vec<(Event, bool)>> {
        let pending = std::mem::take(&mut self.pending);
        if pending.is_empty() {
            return Ok(vec![]);
        }
        let directory = self.catalog.directory.clone();
        check_write_paths(&directory)?;
        fs::create_dir_all(directory.join("tracker"))?;
        check_write_paths(&directory)?;
        let _lock = lock_log(&directory.join(LOCK_FILE))?;
        let mut current = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(error) => return Err(format!("{}: {error}", self.path.display()).into()),
        };
        let parsed = parse_bytes(&current);
        if let Some((line, message)) = parsed.errors.first() {
            return Err(format!("{LOG_FILE}:{line}: {message}; not appending").into());
        }
        if let Some(at) = parsed.torn_at {
            eprintln!(
                "warning: cutting a torn last line from {}",
                self.path.display()
            );
            fs::OpenOptions::new()
                .write(true)
                .open(&self.path)?
                .set_len(at as u64)?;
            current.truncate(at);
        }
        let mut recent: Vec<Event> = parsed.events.into_iter().map(|(_, e)| e).collect();
        let mut keys: BTreeSet<String> = recent.iter().filter_map(|e| e.key.clone()).collect();
        let mut subjects: BTreeSet<String> = recent
            .iter()
            .chain(self.catalog.tracker.log.iter())
            .map(|e| e.subject.clone())
            .collect();
        let mut text = String::new();
        if current.last().is_some_and(|c| *c != b'\n') {
            text.push('\n');
        }
        let mut results = Vec::new();
        for item in pending {
            let event = match item {
                Pending::Event(event) => event,
                Pending::Report { kind, mut event } => {
                    // A retried keyed report: the issue it already created, not a new one.
                    if let Some(existing) = event
                        .key
                        .as_ref()
                        .and_then(|key| recent.iter().find(|e| e.key.as_ref() == Some(key)))
                    {
                        results.push((existing.clone(), false));
                        continue;
                    }
                    let title = event.title.clone().unwrap_or_default();
                    event.subject = self.allocate_report_id(kind, &title, &recent, &subjects);
                    let event = event.with_id();
                    self.check(&event)?;
                    self.catalog.tracker.record(&event);
                    event
                }
            };
            // Only a source key makes an event already recorded.
            let added = event
                .key
                .as_ref()
                .is_none_or(|key| keys.insert(key.clone()));
            if !added {
                // Already recorded: report the recorded event (its id and subject), never
                // the queued copy with its fresh id.
                let existing = recent
                    .iter()
                    .find(|e| e.key.is_some() && e.key == event.key)
                    .cloned()
                    .unwrap_or(event);
                results.push((existing, false));
                continue;
            }
            subjects.insert(event.subject.clone());
            text.push_str(&event.line()?);
            text.push('\n');
            recent.push(event.clone());
            results.push((event, added));
        }
        if results.iter().any(|(_, added)| *added) {
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            file.write_all(text.as_bytes())?;
            file.sync_data()?;
        }
        Ok(results)
    }
}

/// Print the outcome of one append.
pub fn announce(event: &Event, added: bool) {
    let status = if added {
        "recorded"
    } else {
        "already recorded"
    };
    println!(
        "{status}: {} {} ({})",
        event.kind.as_str(),
        event.subject,
        event.id
    );
}

/// Resolve a commit to its full hash when git knows it; otherwise keep it as given.
/// Ambiguous names (a branch and a tag of the same name, say) are kept as given, with a
/// warning; the fold then treats them as unknown.
fn resolve_commit(repo: &Path, commit: &str) -> Result<String> {
    match crate::scan_git::Ancestry::new(repo).resolve(commit) {
        Ok(Some(full)) => Ok(full),
        Ok(None) => Err(format!(
            "--commit {commit:?} does not name exactly one commit (unknown or ambiguous); give a full hash or a full ref such as refs/tags/{commit}"
        )
        .into()),
        Err(error) => Err(format!("--commit {commit:?} cannot be checked: {error}").into()),
    }
}

/// Name an unknown issue before trying to resolve the command's commit.
fn known_issue(writer: &LogWriter, kind: &str, issue: &str) -> Result<()> {
    if writer.known_issue(issue) {
        return Ok(());
    }
    Err(
        format!("{kind} {issue}: {issue} is not an issue in issues.yaml or reported in the log")
            .into(),
    )
}

/// `path` made absolute against `cwd`, with `.` and `..` resolved lexically.
fn lexical_absolute(cwd: &Path, path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in cwd.join(path).components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Refuse writing to a catalog reached through a symlink. The root is the git toplevel of
/// the *canonical* catalog directory (or its canonical parent outside git). Walking the
/// lexical path (absolute, `..` resolved), some ancestor must canonicalize to that root;
/// the deepest such ancestor is the root's lexical counterpart. The path is refused when
/// none does (it enters the repository through a symlink, as with a `spec` symlinked to
/// another checkout), or when any component at or below that counterpart, or below the
/// current directory, is a symlink. Symlinks above both (such as macOS's /var) are fine.
pub fn check_catalog_path(dir: &Path) -> Result<()> {
    if dir
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(format!(
            "catalog {} must not contain `..`; give a path inside the repository instead",
            dir.display()
        )
        .into());
    }
    let cwd = std::env::current_dir()?;
    let lexical = lexical_absolute(&cwd, dir);
    let canonical = fs::canonicalize(&lexical).map_err(|e| format!("{}: {e}", dir.display()))?;
    let root = crate::scan_git::git(&canonical, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(|top| PathBuf::from(top.trim()))
        .filter(|top| !top.as_os_str().is_empty())
        .and_then(|top| fs::canonicalize(top).ok())
        .or_else(|| canonical.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| canonical.clone());
    let Some(counterpart) = lexical
        .ancestors()
        .find(|a| fs::canonicalize(a).is_ok_and(|c| c == root))
    else {
        return Err(format!(
            "refusing to write: catalog path {} reaches {} through a symlink",
            dir.display(),
            root.display()
        )
        .into());
    };
    for ancestor in lexical.ancestors() {
        let checked =
            ancestor.starts_with(counterpart) || (ancestor.starts_with(&cwd) && ancestor != cwd);
        if checked && fs::symlink_metadata(ancestor).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!(
                "refusing to write: catalog path {} goes through symlink {}",
                dir.display(),
                ancestor.display()
            )
            .into());
        }
    }
    Ok(())
}

pub fn run(args: LogArgs) -> Result<()> {
    let dir = catalog_dir(args.catalog)?;
    check_catalog_path(&dir)?;
    let mut writer = LogWriter::open(&dir)?;
    let repo = repo_dir(&writer.catalog);
    let at = match &args.at {
        Some(text) => parse_at(text)?,
        None => now(),
    };
    let by = args.by.clone().unwrap_or_else(|| default_actor(&repo));
    let key = args.key.clone();
    if key.as_deref().is_some_and(|k| k.trim().is_empty()) {
        return Err("--key must not be empty".into());
    }
    let mut event = match args.action {
        LogAction::ScanGit {
            repo: chosen,
            since,
        } => {
            if key.is_some() {
                return Err("scan-git sets its own source keys; leave out --key".into());
            }
            let repo = chosen.unwrap_or(repo);
            return crate::scan_git::scan(&mut writer, &repo, since.as_deref());
        }
        LogAction::Report {
            kind,
            title,
            entries,
            platforms,
            impact,
            source,
        } => {
            let kind = match kind {
                ReportKind::Bug => IssueKind::Bug,
                ReportKind::Request => IssueKind::Request,
            };
            let mut event = Event::new(EventKind::Reported, "", &at, &by);
            event.title = Some(title);
            event.entries = entries;
            event.platforms = platforms;
            event.impact = impact.map(|i| match i {
                ImpactArg::Critical => Impact::Critical,
                ImpactArg::High => Impact::High,
                ImpactArg::Medium => Impact::Medium,
                ImpactArg::Low => Impact::Low,
            });
            event.source = source;
            event.key = key;
            writer.report(kind, event)?;
            let results = writer.flush()?;
            for (event, added) in &results {
                announce(event, *added);
            }
            return Ok(());
        }
        LogAction::Fixed { issue, commit } => {
            known_issue(&writer, "fixed", &issue)?;
            let mut event = Event::new(EventKind::Fixed, &issue, &at, &by);
            event.commit = Some(resolve_commit(&repo, &commit)?);
            event
        }
        LogAction::Verified {
            issue,
            evidence,
            commit,
        } => {
            known_issue(&writer, "verified", &issue)?;
            if args.by.is_none() {
                return Err("log verified needs --by: verification counts only when someone other than the fixer records it".into());
            }
            let mut event = Event::new(EventKind::Verified, &issue, &at, &by);
            event.evidence = evidence;
            event.commit = commit.map(|c| resolve_commit(&repo, &c)).transpose()?;
            event
        }
        LogAction::Reopened { issue, reason } => {
            let mut event = Event::new(EventKind::Reopened, &issue, &at, &by);
            event.reason = Some(reason);
            event
        }
        LogAction::Uploaded {
            release,
            platform,
            build,
            commit,
        } => {
            let mut event = Event::new(EventKind::Uploaded, &release, &at, &by);
            event.platform = Some(platform);
            event.build = Some(build);
            event.commit = commit.map(|c| resolve_commit(&repo, &c)).transpose()?;
            event
        }
        LogAction::Released {
            release,
            platform,
            date,
            commit,
        } => {
            let mut event = Event::new(EventKind::Released, &release, &at, &by);
            event.platform = platform;
            event.date = Some(date.unwrap_or_else(|| day(&at)));
            event.commit = commit.map(|c| resolve_commit(&repo, &c)).transpose()?;
            event
        }
        LogAction::Pulled { release, reason } => {
            let mut event = Event::new(EventKind::Pulled, &release, &at, &by);
            event.reason = Some(reason);
            event
        }
    };
    event.key = key;
    let event = event.with_id();
    if !writer.append(&event)? {
        announce(&writer.recorded_with_key(&event), false);
        return Ok(());
    }
    for (event, added) in writer.flush()? {
        announce(&event, added);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slugs() {
        assert_eq!(
            slug("Home stays blank after recovery!"),
            "home-stays-blank-after-recovery"
        );
        assert_eq!(slug("  Café — crash  "), "caf-crash");
        assert_eq!(slug("***"), "untitled");
        let long =
            slug("Playback stops when the phone locks during a long episode download on cellular");
        assert!(long.len() <= SLUG_MAX, "{long}");
        assert!(!long.ends_with('-'));
        assert_eq!(long, "playback-stops-when-the-phone-locks-during-a");
    }
    #[test]
    fn timestamps() {
        assert_eq!(timestamp_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp_from_unix(1_795_000_000), "2026-11-18T11:06:40Z");
        assert!(valid_timestamp("2026-11-22T23:59:59Z"));
        assert!(!valid_timestamp("2026-11-22T24:00:00Z"));
        assert!(!valid_timestamp("2026-11-22 10:00:00Z"));
        assert!(!valid_timestamp("2026-02-30T10:00:00Z"));
    }
    #[test]
    fn fnv() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
