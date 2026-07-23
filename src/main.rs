use anyhow::{Context, Result};
use chrono::{DateTime, Local, TimeZone};
use clap::Parser;
use crossterm::{
    cursor::MoveTo,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    style::{Color, Stylize},
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType},
};
use notify::{Event as NotifyEvent, EventKind, RecursiveMode, Watcher};
use regex::Regex;
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, IsTerminal, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, Receiver, RecvTimeoutError, Sender},
        Arc,
    },
    time::Duration,
};

mod config;
use config::Config;

/// Lines retained for `/` search when nothing else is configured. At a typical
/// log line this is single-digit megabytes.
const DEFAULT_HISTORY_LIMIT: usize = 10_000;

/// Most matches a single search will replay, so a broad pattern cannot flood
/// the terminal with the entire history.
const SEARCH_RESULT_LIMIT: usize = 100;

const BANNER: &str = "LogWatch - 'q' quit  'c' clear  'p' pause  '/' search  's' stats";
const RULE: &str = "─────────────────────────────────────────────────────────────────────";

/// Default time-bucket size for activity stats, in seconds.
const DEFAULT_STATS_INTERVAL_SECS: i64 = 60;

/// Cap on retained stats buckets so a long-running monitor stays bounded. At
/// the default 60s interval this is well over a day of history.
const STATS_MAX_BUCKETS: usize = 2000;

/// Most recent buckets drawn in the `s` histogram.
const STATS_HISTOGRAM_ROWS: usize = 20;

/// Character width of a full activity bar in the histogram.
const STATS_BAR_WIDTH: usize = 32;

/// Files listed in the histogram's "by file" summary.
const STATS_TOP_FILES: usize = 10;

/// Character width of a full bar in the "by file" summary.
const STATS_FILE_BAR_WIDTH: usize = 24;

#[derive(Parser, Debug)]
#[command(name = "logwatch")]
#[command(version)]
#[command(about = "A real-time log file monitoring tool", long_about = None)]
struct Args {
    /// Directory to watch (can be specified multiple times)
    #[arg(short, long, value_name = "DIR")]
    directory: Vec<PathBuf>,

    /// Configuration file path
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Use full absolute paths instead of relative
    #[arg(short = 'P', long)]
    full_paths: bool,

    /// Include pattern (regex)
    #[arg(short, long, value_name = "PATTERN")]
    include: Vec<String>,

    /// Exclude pattern (regex)
    #[arg(short, long, value_name = "PATTERN")]
    exclude: Vec<String>,

    /// Show NUM lines of leading context before each match
    #[arg(
        short = 'B',
        long = "before-context",
        value_name = "NUM",
        default_value_t = 0
    )]
    before_context: usize,

    /// Show NUM lines of trailing context after each match
    #[arg(
        short = 'A',
        long = "after-context",
        value_name = "NUM",
        default_value_t = 0
    )]
    after_context: usize,

    /// Show NUM lines of output context around each match (sets both -A and -B unless explicitly set)
    #[arg(short = 'C', long = "context", value_name = "NUM")]
    context: Option<usize>,

    /// File pattern to watch (e.g., "*.log")
    #[arg(short = 'f', long, value_name = "PATTERN", default_value = "*.log")]
    file_pattern: String,

    /// Read from stdin instead of watching files (for remote log streaming)
    #[arg(long)]
    stdin: bool,

    /// Also write displayed lines to FILE (plain text, no color)
    #[arg(short = 'o', long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// Append to the output file instead of overwriting it
    #[arg(long, requires = "output")]
    append: bool,

    /// Lines to retain in memory for '/' search (0 disables history)
    #[arg(long, value_name = "NUM")]
    history: Option<usize>,

    /// Time-bucket size in seconds for the activity stats ('s' key) [default: 60]
    #[arg(long, value_name = "SECS")]
    stats_interval: Option<i64>,

    /// Write an activity-stats report to FILE at exit (.json for JSON, else CSV)
    #[arg(long, value_name = "FILE")]
    stats_out: Option<PathBuf>,

    /// Disable colored output
    #[arg(long)]
    no_color: bool,
}

/// How file paths are rendered in the output stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathStyle {
    /// Absolute path, e.g. `/var/log/app/error.log`
    Full,
    /// Path relative to the matching watch root, e.g. `app/error.log`
    Relative,
    /// Filename only, e.g. `error.log`
    Filename,
}

impl PathStyle {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "full" => Ok(Self::Full),
            "relative" => Ok(Self::Relative),
            "filename" => Ok(Self::Filename),
            other => anyhow::bail!(
                "Invalid path_style {:?}: expected \"full\", \"relative\", or \"filename\"",
                other
            ),
        }
    }

    fn render(self, path: &Path, watch_roots: &[PathBuf]) -> String {
        match self {
            Self::Full => path.display().to_string(),
            Self::Filename => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            Self::Relative => watch_roots
                .iter()
                .find_map(|root| path.strip_prefix(root).ok())
                .unwrap_or(path)
                .display()
                .to_string(),
        }
    }
}

#[derive(Clone)]
enum LogMessage {
    Line {
        path: PathBuf,
        line: String,
        timestamp: chrono::DateTime<Local>,
    },
    Info(String),
}

struct FileTracker {
    readers: HashMap<PathBuf, (BufReader<File>, u64)>,
    watch_roots: Vec<PathBuf>,
}

impl FileTracker {
    fn new(watch_roots: Vec<PathBuf>) -> Self {
        Self {
            readers: HashMap::new(),
            watch_roots,
        }
    }

    fn handle_file_change(&mut self, path: &PathBuf, tx: &Sender<LogMessage>) -> Result<()> {
        let metadata = std::fs::metadata(path)?;
        let current_size = metadata.len();

        if let Some((reader, last_pos)) = self.readers.get_mut(path) {
            // File was truncated (log rotation)
            if current_size < *last_pos {
                *reader = BufReader::new(File::open(path)?);
                *last_pos = 0;
            }

            reader.seek(SeekFrom::Start(*last_pos))?;
            let mut line = String::new();

            while reader.read_line(&mut line)? > 0 {
                if !line.trim().is_empty() {
                    tx.send(LogMessage::Line {
                        path: path.clone(),
                        line: line.trim_end().to_string(),
                        timestamp: Local::now(),
                    })?;
                }
                line.clear();
            }

            *last_pos = reader.stream_position()?;
        } else {
            // New file - read existing content and start tracking
            let mut file = File::open(path)?;
            file.seek(SeekFrom::End(0))?;
            let pos = file.stream_position()?;
            let reader = BufReader::new(file);
            self.readers.insert(path.clone(), (reader, pos));

            tx.send(LogMessage::Info(format!(
                "Now watching: {}",
                path.display()
            )))?;
        }

        Ok(())
    }

    fn discover_files(&mut self, pattern: &str, tx: &Sender<LogMessage>) -> Result<()> {
        let glob_pattern = glob::Pattern::new(pattern)?;

        for root in &self.watch_roots.clone() {
            if let Ok(entries) = std::fs::read_dir(root) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(name) = path.file_name() {
                            if glob_pattern.matches_path(Path::new(name)) {
                                let _ = self.handle_file_change(&path, tx);
                            }
                        }
                    } else if path.is_dir() {
                        self.discover_files_recursive(&path, pattern, tx)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn discover_files_recursive(
        &mut self,
        dir: &Path,
        pattern: &str,
        tx: &Sender<LogMessage>,
    ) -> Result<()> {
        let glob_pattern = glob::Pattern::new(pattern)?;

        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(name) = path.file_name() {
                        if glob_pattern.matches_path(Path::new(name)) {
                            let _ = self.handle_file_change(&path, tx);
                        }
                    }
                } else if path.is_dir() {
                    self.discover_files_recursive(&path, pattern, tx)?;
                }
            }
        }
        Ok(())
    }
}

struct LogFilter {
    include: Vec<Regex>,
    exclude: Vec<Regex>,
}

impl LogFilter {
    fn new(include: Vec<String>, exclude: Vec<String>) -> Result<Self> {
        let include = include
            .into_iter()
            .map(|p| Regex::new(&p))
            .collect::<Result<Vec<_>, _>>()
            .context("Invalid include pattern")?;

        let exclude = exclude
            .into_iter()
            .map(|p| Regex::new(&p))
            .collect::<Result<Vec<_>, _>>()
            .context("Invalid exclude pattern")?;

        Ok(Self { include, exclude })
    }

    fn matches(&self, line: &str) -> bool {
        // If there are include patterns, line must match at least one
        if !self.include.is_empty() && !self.include.iter().any(|re| re.is_match(line)) {
            return false;
        }

        // Line must not match any exclude patterns
        !self.exclude.iter().any(|re| re.is_match(line))
    }
}

fn watch_files(
    directories: Vec<PathBuf>,
    file_pattern: String,
    tx: Sender<LogMessage>,
) -> Result<()> {
    let (notify_tx, notify_rx) = channel();

    let mut watcher = notify::recommended_watcher(notify_tx)?;

    for dir in &directories {
        watcher.watch(dir, RecursiveMode::Recursive)?;
    }

    let mut tracker = FileTracker::new(directories);
    tracker.discover_files(&file_pattern, &tx)?;

    loop {
        match notify_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(event)) => handle_notify_event(event, &mut tracker, &tx, &file_pattern)?,
            Ok(Err(e)) => eprintln!("Watch error: {:?}", e),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(())
}

fn handle_notify_event(
    event: NotifyEvent,
    tracker: &mut FileTracker,
    tx: &Sender<LogMessage>,
    file_pattern: &str,
) -> Result<()> {
    let glob_pattern = glob::Pattern::new(file_pattern)?;

    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) => {
            for path in event.paths {
                if path.is_file() {
                    if let Some(name) = path.file_name() {
                        if glob_pattern.matches_path(Path::new(name)) {
                            let _ = tracker.handle_file_change(&path, tx);
                        }
                    }
                }
            }
        }
        _ => {}
    }

    Ok(())
}

/// Everything `display_logs` needs beyond the message stream itself.
struct DisplayOptions {
    path_style: PathStyle,
    before_context: usize,
    after_context: usize,
    use_color: bool,
    history_limit: usize,
    output: Option<PathBuf>,
    append: bool,
    stats_interval: i64,
    stats_out: Option<PathBuf>,
}

/// Severity parsed out of a log line. Retained alongside each history entry so
/// search results keep their coloring without re-parsing the message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogLevel {
    Fatal,
    Error,
    Warn,
    Info,
    Debug,
    Other,
}

impl LogLevel {
    fn detect(line: &str) -> Self {
        let upper = line.to_ascii_uppercase();
        if upper.contains("FATAL") || upper.contains("CRITICAL") || upper.contains("PANIC") {
            Self::Fatal
        } else if upper.contains("ERROR") || upper.contains("ERR") {
            Self::Error
        } else if upper.contains("WARN") || upper.contains("WARNING") {
            Self::Warn
        } else if upper.contains("INFO") {
            Self::Info
        } else if upper.contains("DEBUG") || upper.contains("TRACE") {
            Self::Debug
        } else {
            Self::Other
        }
    }

    fn color(self) -> Option<Color> {
        match self {
            Self::Fatal => Some(Color::Red),
            Self::Error => Some(Color::DarkRed),
            Self::Warn => Some(Color::Yellow),
            Self::Info => Some(Color::Blue),
            Self::Debug => Some(Color::DarkGrey),
            Self::Other => None,
        }
    }

    /// Every variant, in a fixed order. Used to index the per-level count arrays
    /// in the stats buckets.
    const ALL: [LogLevel; 6] = [
        Self::Fatal,
        Self::Error,
        Self::Warn,
        Self::Info,
        Self::Debug,
        Self::Other,
    ];

    /// Number of variants; the width of a per-level count array.
    const COUNT: usize = Self::ALL.len();

    /// Position of this level in `ALL`, for array indexing.
    fn index(self) -> usize {
        Self::ALL.iter().position(|&l| l == self).unwrap()
    }

    /// Lowercase label used in stats reports (CSV/JSON columns).
    fn label(self) -> &'static str {
        match self {
            Self::Fatal => "fatal",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Other => "other",
        }
    }
}

/// Counts for one time bucket, keyed by display path then by level. Stores
/// counts only - never the lines - so a long-running monitor stays cheap.
#[derive(Default)]
struct StatsBucket {
    by_file: HashMap<String, [u64; LogLevel::COUNT]>,
}

impl StatsBucket {
    fn record(&mut self, level: LogLevel, file: &str) {
        let counts = self
            .by_file
            .entry(file.to_string())
            .or_insert([0; LogLevel::COUNT]);
        counts[level.index()] += 1;
    }

    fn total(&self) -> u64 {
        self.by_file.values().flat_map(|c| c.iter()).sum()
    }

    /// Per-level totals across every file in this bucket.
    fn by_level(&self) -> [u64; LogLevel::COUNT] {
        let mut out = [0u64; LogLevel::COUNT];
        for counts in self.by_file.values() {
            for (slot, c) in out.iter_mut().zip(counts) {
                *slot += c;
            }
        }
        out
    }
}

/// Activity counters bucketed by arrival time. Feeds the `s` histogram and the
/// `--stats-out` report. Bounded to `max_buckets` (oldest dropped first).
struct Stats {
    interval_secs: i64,
    max_buckets: usize,
    /// Keyed by bucket-start epoch seconds, ordered oldest-first.
    buckets: BTreeMap<i64, StatsBucket>,
}

impl Stats {
    fn new(interval_secs: i64, max_buckets: usize) -> Self {
        Self {
            interval_secs: interval_secs.max(1),
            max_buckets: max_buckets.max(1),
            buckets: BTreeMap::new(),
        }
    }

    /// Floor a timestamp to the start of its bucket (epoch seconds).
    fn bucket_key(&self, ts: DateTime<Local>) -> i64 {
        ts.timestamp().div_euclid(self.interval_secs) * self.interval_secs
    }

    fn record(&mut self, ts: DateTime<Local>, level: LogLevel, file: &str) {
        let key = self.bucket_key(ts);
        self.buckets.entry(key).or_default().record(level, file);

        // Bound memory: drop the oldest bucket(s) once over the cap.
        while self.buckets.len() > self.max_buckets {
            let Some(&oldest) = self.buckets.keys().next() else {
                break;
            };
            self.buckets.remove(&oldest);
        }
    }

    fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }

    fn total_lines(&self) -> u64 {
        self.buckets.values().map(StatsBucket::total).sum()
    }

    /// Per-level totals across all retained buckets.
    fn level_totals(&self) -> [u64; LogLevel::COUNT] {
        let mut out = [0u64; LogLevel::COUNT];
        for bucket in self.buckets.values() {
            for (slot, c) in out.iter_mut().zip(bucket.by_level()) {
                *slot += c;
            }
        }
        out
    }

    /// Per-file totals across all retained buckets, highest first.
    fn file_totals(&self) -> Vec<(String, u64)> {
        let mut totals: HashMap<String, u64> = HashMap::new();
        for bucket in self.buckets.values() {
            for (file, counts) in &bucket.by_file {
                *totals.entry(file.clone()).or_insert(0) += counts.iter().sum::<u64>();
            }
        }
        let mut totals: Vec<_> = totals.into_iter().collect();
        // Sort by count desc, then name asc for a stable, readable order.
        totals.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        totals
    }

    fn bucket_label(&self, key: i64) -> String {
        match Local.timestamp_opt(key, 0).single() {
            Some(t) => t.format("%H:%M").to_string(),
            None => key.to_string(),
        }
    }

    /// A severity-stacked bar for one bucket, scaled so `max_total` fills
    /// `STATS_BAR_WIDTH`. Error/fatal render as `▓`, warn as `▒`, the rest `░`.
    fn bar(&self, bucket: &StatsBucket, max_total: u64) -> String {
        let total = bucket.total();
        if total == 0 || max_total == 0 {
            return String::new();
        }
        let width = ((total as f64 / max_total as f64) * STATS_BAR_WIDTH as f64).round() as usize;
        if width == 0 {
            return String::new();
        }

        let levels = bucket.by_level();
        let err = levels[LogLevel::Fatal.index()] + levels[LogLevel::Error.index()];
        let warn = levels[LogLevel::Warn.index()];

        let err_len = ((err as f64 / total as f64) * width as f64).round() as usize;
        let err_len = err_len.min(width);
        let warn_len = ((warn as f64 / total as f64) * width as f64).round() as usize;
        let warn_len = warn_len.min(width - err_len);
        let other_len = width - err_len - warn_len;

        format!(
            "{}{}{}",
            "▓".repeat(err_len),
            "▒".repeat(warn_len),
            "░".repeat(other_len),
        )
    }

    /// The multi-line histogram shown when the user presses `s`.
    fn render_report(&self) -> Vec<String> {
        if self.is_empty() {
            return vec!["-- no activity recorded yet --".to_string()];
        }

        let first = *self.buckets.keys().next().unwrap();
        let last = *self.buckets.keys().next_back().unwrap();
        let span = match (
            Local.timestamp_opt(first, 0).single(),
            Local.timestamp_opt(last, 0).single(),
        ) {
            (Some(s), Some(e)) => format!(
                "{} {}-{}",
                s.format("%Y-%m-%d"),
                s.format("%H:%M"),
                e.format("%H:%M")
            ),
            _ => String::new(),
        };

        let mut out = vec![
            format!(
                "── Activity ── {}s buckets ── {} ──",
                self.interval_secs, span
            ),
            "   (▓ error/fatal  ▒ warn  ░ info/debug/other)".to_string(),
        ];

        // Most recent buckets, drawn oldest-to-newest.
        let recent: Vec<(&i64, &StatsBucket)> = self
            .buckets
            .iter()
            .rev()
            .take(STATS_HISTOGRAM_ROWS)
            .collect();
        let max_total = recent.iter().map(|(_, b)| b.total()).max().unwrap_or(0);
        for (key, bucket) in recent.into_iter().rev() {
            out.push(format!(
                "{}  {:<width$}  {}",
                self.bucket_label(*key),
                self.bar(bucket, max_total),
                bucket.total(),
                width = STATS_BAR_WIDTH,
            ));
        }

        // Totals line, nonzero levels only.
        let levels = self.level_totals();
        let breakdown: Vec<String> = LogLevel::ALL
            .iter()
            .filter(|l| levels[l.index()] > 0)
            .map(|l| format!("{} {}", l.label(), levels[l.index()]))
            .collect();
        let breakdown = if breakdown.is_empty() {
            String::new()
        } else {
            format!(" — {}", breakdown.join(", "))
        };
        out.push(format!("Totals: {} lines{}", self.total_lines(), breakdown));

        // By-file summary.
        let files = self.file_totals();
        if !files.is_empty() {
            out.push("By file:".to_string());
            let max_file = files.first().map(|(_, c)| *c).unwrap_or(0).max(1);
            for (file, count) in files.iter().take(STATS_TOP_FILES) {
                let len = ((*count as f64 / max_file as f64) * STATS_FILE_BAR_WIDTH as f64).round()
                    as usize;
                out.push(format!(
                    "  {:<28} {:>8}  {}",
                    truncate_end(file, 28),
                    count,
                    "█".repeat(len),
                ));
            }
            if files.len() > STATS_TOP_FILES {
                out.push(format!(
                    "  … and {} more files",
                    files.len() - STATS_TOP_FILES
                ));
            }
        }

        out
    }

    /// Serialize the report and write it to `path`; `.json` selects JSON,
    /// anything else CSV.
    fn write_report(&self, path: &Path) -> Result<()> {
        let is_json = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
        let body = if is_json {
            self.to_json()?
        } else {
            self.to_csv()
        };
        std::fs::write(path, body)
            .with_context(|| format!("Failed to write stats report: {}", path.display()))
    }

    /// Tidy long-format CSV: one row per (bucket, file, level) with a nonzero
    /// count. Easy to pivot in a spreadsheet or plotting tool.
    fn to_csv(&self) -> String {
        let mut out = String::from("bucket_start,file,level,count\n");
        for (key, bucket) in &self.buckets {
            let start = Local
                .timestamp_opt(*key, 0)
                .single()
                .map(|t| t.format("%Y-%m-%dT%H:%M:%S%:z").to_string())
                .unwrap_or_else(|| key.to_string());
            // Stable ordering: file name, then level order.
            let mut files: Vec<_> = bucket.by_file.iter().collect();
            files.sort_by(|a, b| a.0.cmp(b.0));
            for (file, counts) in files {
                for level in LogLevel::ALL {
                    let count = counts[level.index()];
                    if count > 0 {
                        out.push_str(&format!(
                            "{},{},{},{}\n",
                            csv_field(&start),
                            csv_field(file),
                            level.label(),
                            count,
                        ));
                    }
                }
            }
        }
        out
    }

    fn to_json(&self) -> Result<String> {
        let buckets: Vec<StatsBucketReport> = self
            .buckets
            .iter()
            .map(|(key, bucket)| {
                let start = Local
                    .timestamp_opt(*key, 0)
                    .single()
                    .map(|t| t.format("%Y-%m-%dT%H:%M:%S%:z").to_string())
                    .unwrap_or_else(|| key.to_string());

                let levels = bucket.by_level();
                let by_level: BTreeMap<String, u64> = LogLevel::ALL
                    .iter()
                    .filter(|l| levels[l.index()] > 0)
                    .map(|l| (l.label().to_string(), levels[l.index()]))
                    .collect();

                let mut by_file: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
                for (file, counts) in &bucket.by_file {
                    let per_level: BTreeMap<String, u64> = LogLevel::ALL
                        .iter()
                        .filter(|l| counts[l.index()] > 0)
                        .map(|l| (l.label().to_string(), counts[l.index()]))
                        .collect();
                    by_file.insert(file.clone(), per_level);
                }

                StatsBucketReport {
                    start,
                    total: bucket.total(),
                    by_level,
                    by_file,
                }
            })
            .collect();

        let report = StatsReport {
            interval_seconds: self.interval_secs,
            generated_at: Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string(),
            total_lines: self.total_lines(),
            buckets,
        };
        serde_json::to_string_pretty(&report).context("Failed to serialize stats to JSON")
    }
}

#[derive(Serialize)]
struct StatsReport {
    interval_seconds: i64,
    generated_at: String,
    total_lines: u64,
    buckets: Vec<StatsBucketReport>,
}

#[derive(Serialize)]
struct StatsBucketReport {
    start: String,
    total: u64,
    by_level: BTreeMap<String, u64>,
    by_file: BTreeMap<String, BTreeMap<String, u64>>,
}

/// Quote a CSV field if it contains a comma, quote, or newline (RFC 4180).
fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Trim a string to `max` characters, marking the cut with a leading `…` so the
/// distinctive tail of a path stays visible.
fn truncate_end(value: &str, max: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max {
        return value.to_string();
    }
    let tail: String = chars[chars.len() - (max - 1)..].iter().collect();
    format!("…{}", tail)
}

/// A retained line. Stores the fully rendered text so a search can match on the
/// timestamp and file path as well as the message body.
struct HistoryEntry {
    rendered: String,
    level: LogLevel,
}

/// Bounded ring buffer of every line seen - *including* lines the filters hid -
/// so a search can surface something the active filter would have dropped.
struct History {
    entries: VecDeque<HistoryEntry>,
    limit: usize,
}

impl History {
    fn new(limit: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            limit,
        }
    }

    fn is_enabled(&self) -> bool {
        self.limit > 0
    }

    fn push(&mut self, rendered: &str, level: LogLevel) {
        if !self.is_enabled() {
            return;
        }
        while self.entries.len() >= self.limit {
            self.entries.pop_front();
        }
        self.entries.push_back(HistoryEntry {
            rendered: rendered.to_string(),
            level,
        });
    }

    fn search(&self, pattern: &Regex) -> Vec<&HistoryEntry> {
        self.entries
            .iter()
            .filter(|entry| pattern.is_match(&entry.rendered))
            .collect()
    }
}

/// Writes the stream to the terminal and, when `--output` is set, mirrors log
/// lines to a file. Terminal chrome and ANSI color never reach the file.
struct Output {
    /// Boxed so tests can capture the terminal side instead of writing to the
    /// real stdout.
    term: Box<dyn Write>,
    raw_mode: bool,
    use_color: bool,
    file: Option<BufWriter<File>>,
    file_dirty: bool,
}

impl Output {
    fn new(
        term: Box<dyn Write>,
        path: Option<&Path>,
        append: bool,
        use_color: bool,
    ) -> Result<Self> {
        let file = match path {
            Some(path) => {
                let mut options = OpenOptions::new();
                options.write(true).create(true);
                if append {
                    options.append(true);
                } else {
                    options.truncate(true);
                }
                let file = options
                    .open(path)
                    .with_context(|| format!("Failed to open output file: {}", path.display()))?;
                Some(BufWriter::new(file))
            }
            None => None,
        };

        Ok(Self {
            term,
            raw_mode: false,
            use_color,
            file,
            file_dirty: false,
        })
    }

    /// Raw mode leaves the cursor where it is on a bare newline, so every line
    /// needs an explicit carriage return.
    fn write_term(&mut self, text: &str) -> Result<()> {
        if self.raw_mode {
            write!(self.term, "{}\r\n", text)?;
        } else {
            writeln!(self.term, "{}", text)?;
        }
        Ok(())
    }

    /// Terminal-only chrome: banners, `[INFO]` notices, search headers.
    fn notice(&mut self, text: &str) -> Result<()> {
        self.write_term(text)
    }

    /// A log line shown on the terminal but not recorded - used for search
    /// results, which are replays of lines the export file already holds.
    fn echo(&mut self, rendered: &str, level: LogLevel) -> Result<()> {
        let text = match level.color() {
            Some(color) if self.use_color => rendered.with(color).to_string(),
            _ => rendered.to_string(),
        };
        self.write_term(&text)
    }

    /// A log line from the live stream: terminal plus the export file.
    fn log(&mut self, rendered: &str, level: LogLevel) -> Result<()> {
        self.echo(rendered, level)?;
        if let Some(file) = self.file.as_mut() {
            writeln!(file, "{}", rendered)?;
            self.file_dirty = true;
        }
        Ok(())
    }

    /// Called whenever the stream goes quiet, so `tail -f` on the export file
    /// sees data promptly without paying a flush per line under load.
    fn flush(&mut self) -> Result<()> {
        self.term.flush()?;
        if self.file_dirty {
            if let Some(file) = self.file.as_mut() {
                file.flush()?;
            }
            self.file_dirty = false;
        }
        Ok(())
    }
}

/// Reads a pattern at the `/` prompt. Returns `None` if the user cancelled or
/// submitted an empty pattern.
fn prompt_for_search(output: &mut Output) -> Result<Option<String>> {
    let mut query = String::new();
    redraw_prompt(output, &query)?;

    loop {
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => break,
            (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                clear_prompt(output)?;
                return Ok(None);
            }
            (KeyCode::Backspace, _) => {
                query.pop();
            }
            (KeyCode::Char(c), modifiers) if !modifiers.contains(KeyModifiers::CONTROL) => {
                query.push(c);
            }
            _ => {}
        }

        redraw_prompt(output, &query)?;
    }

    clear_prompt(output)?;
    Ok(if query.is_empty() { None } else { Some(query) })
}

fn redraw_prompt(output: &mut Output, query: &str) -> Result<()> {
    execute!(output.term, Clear(ClearType::CurrentLine))?;
    write!(output.term, "\rsearch: /{}", query)?;
    output.term.flush()?;
    Ok(())
}

fn clear_prompt(output: &mut Output) -> Result<()> {
    execute!(output.term, Clear(ClearType::CurrentLine))?;
    write!(output.term, "\r")?;
    output.term.flush()?;
    Ok(())
}

/// Runs `pattern` against the retained history and replays the matches inline.
fn run_search(output: &mut Output, history: &History, pattern: &str) -> Result<()> {
    if !history.is_enabled() {
        return output.notice("-- search unavailable: history is disabled (--history 0) --");
    }

    let regex = match Regex::new(pattern) {
        Ok(regex) => regex,
        Err(err) => {
            // Regex errors are multi-line, which wrecks the layout in raw mode.
            let reason = err.to_string().replace('\n', " ");
            return output.notice(&format!("-- invalid search pattern: {} --", reason));
        }
    };

    let retained = history.entries.len();
    let matches = history.search(&regex);

    if matches.is_empty() {
        return output.notice(&format!(
            "-- no matches for /{} in {} retained lines --",
            pattern, retained
        ));
    }

    let shown = matches.len().min(SEARCH_RESULT_LIMIT);
    let header = if shown < matches.len() {
        format!(
            "-- most recent {} of {} matches for /{} ({} lines retained) --",
            shown,
            matches.len(),
            pattern,
            retained
        )
    } else {
        let noun = if matches.len() == 1 {
            "match"
        } else {
            "matches"
        };
        format!(
            "-- {} {} for /{} ({} lines retained) --",
            matches.len(),
            noun,
            pattern,
            retained
        )
    };

    output.notice(&header)?;
    for entry in &matches[matches.len() - shown..] {
        output.echo(&entry.rendered, entry.level)?;
    }
    output.notice("-- end of matches, streaming resumed --")
}

fn display_logs(
    rx: Receiver<LogMessage>,
    filter: LogFilter,
    watch_roots: &[PathBuf],
    options: DisplayOptions,
) -> Result<()> {
    let mut output = Output::new(
        Box::new(io::stdout()),
        options.output.as_deref(),
        options.append,
        options.use_color,
    )?;

    // Keyboard input comes from the controlling terminal rather than stdin, so
    // the interactive keys keep working while stdin carries a log stream
    // (`ssh ... | logwatch --stdin`). Degrade quietly when there is no terminal.
    let interactive = io::stdout().is_terminal() && enable_raw_mode().is_ok();
    output.raw_mode = interactive;

    // Break out of the loop on Ctrl-C / SIGTERM so the exit-time stats report
    // and buffered output flush cleanly. In interactive raw mode the kernel
    // does not raise SIGINT (Ctrl-C arrives as a key event, handled in the
    // loop); this handler mainly covers headless runs and `kill`.
    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let shutdown = Arc::clone(&shutdown);
        let _ = ctrlc::set_handler(move || shutdown.store(true, Ordering::SeqCst));
    }

    if interactive {
        execute!(output.term, Clear(ClearType::All), MoveTo(0, 0))?;
        output.notice(BANNER)?;
    } else {
        output.notice("LogWatch - non-interactive mode (no terminal attached)")?;
    }
    output.notice(RULE)?;

    if let Some(path) = options.output.as_deref() {
        let mode = if options.append {
            "appending"
        } else {
            "writing"
        };
        output.notice(&format!("[INFO] {} output to: {}", mode, path.display()))?;
    }

    // Run the loop separately so a mid-stream error can never leave the user's
    // terminal stuck in raw mode.
    let result = run_display_loop(
        &rx,
        &filter,
        watch_roots,
        &options,
        &mut output,
        interactive,
        &shutdown,
    );

    if interactive {
        disable_raw_mode()?;
    }
    output.flush()?;
    result
}

fn run_display_loop(
    rx: &Receiver<LogMessage>,
    filter: &LogFilter,
    watch_roots: &[PathBuf],
    options: &DisplayOptions,
    output: &mut Output,
    interactive: bool,
    shutdown: &Arc<AtomicBool>,
) -> Result<()> {
    let mut history = History::new(options.history_limit);
    let mut stats = Stats::new(options.stats_interval, STATS_MAX_BUCKETS);
    let mut paused = false;
    let mut stream_ended = false;
    let mut line_number: u64 = 0;
    let mut last_printed_line: u64 = 0;
    let mut trailing_context_remaining = 0usize;
    let mut previous_lines: VecDeque<(u64, String, LogLevel)> = VecDeque::new();

    loop {
        // Ctrl-C / SIGTERM: leave the loop so stats and output flush on the way
        // out instead of the process being torn down mid-write.
        if shutdown.load(Ordering::SeqCst) {
            if interactive {
                output.notice("-- shutting down --")?;
            }
            break;
        }

        if interactive && event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                // Windows reports both press and release; only act on press.
                if key.kind == KeyEventKind::Press {
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                            break
                        }
                        (KeyCode::Char('c'), _) => {
                            execute!(output.term, Clear(ClearType::All), MoveTo(0, 0))?;
                            output.notice(BANNER)?;
                            output.notice(RULE)?;
                        }
                        (KeyCode::Char('p'), _) => {
                            paused = !paused;
                            let status = if paused {
                                "PAUSED - incoming lines are buffered"
                            } else {
                                "RESUMED"
                            };
                            output.notice(&format!("-- {} --", status))?;
                        }
                        (KeyCode::Char('/'), _) => {
                            if let Some(pattern) = prompt_for_search(output)? {
                                run_search(output, &history, &pattern)?;
                            }
                        }
                        (KeyCode::Char('s'), _) => {
                            for line in stats.render_report() {
                                output.notice(&line)?;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // While paused we stop draining the channel entirely, so lines queue up
        // and are shown on resume rather than being dropped on the floor.
        if paused {
            continue;
        }

        // The producer is gone, but the retained history is still worth having
        // around, so keep serving keys until the user quits.
        if stream_ended {
            continue;
        }

        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(LogMessage::Line {
                path,
                line,
                timestamp,
            }) => {
                line_number += 1;

                let display_path = options.path_style.render(&path, watch_roots);
                let rendered = format!(
                    "[{}] {}: {}",
                    timestamp.format("%Y-%m-%d %H:%M:%S"),
                    display_path,
                    line
                );
                let level = LogLevel::detect(&line);

                // Retain every line, filtered out or not, so `/` search and the
                // activity stats reflect real volume, not the filtered view.
                history.push(&rendered, level);
                stats.record(timestamp, level, &display_path);

                if filter.matches(&line) {
                    for (number, text, context_level) in previous_lines.iter() {
                        if *number > last_printed_line {
                            output.log(text, *context_level)?;
                            last_printed_line = *number;
                        }
                    }

                    if line_number > last_printed_line {
                        output.log(&rendered, level)?;
                        last_printed_line = line_number;
                    }

                    trailing_context_remaining = options.after_context;
                } else if trailing_context_remaining > 0 {
                    if line_number > last_printed_line {
                        output.log(&rendered, level)?;
                        last_printed_line = line_number;
                    }
                    trailing_context_remaining -= 1;
                }

                if options.before_context > 0 {
                    previous_lines.push_back((line_number, rendered, level));
                    if previous_lines.len() > options.before_context {
                        previous_lines.pop_front();
                    }
                }
            }
            Ok(LogMessage::Info(info)) => output.notice(&format!("[INFO] {}", info))?,
            Err(RecvTimeoutError::Timeout) => output.flush()?,
            Err(RecvTimeoutError::Disconnected) => {
                output.flush()?;
                // Without a terminal there is nobody to search the history, so
                // a finished stream simply means we are done.
                if !interactive {
                    break;
                }
                output.notice(
                    "-- input stream ended, history still searchable, press 'q' to quit --",
                )?;
                stream_ended = true;
            }
        }
    }

    finalize_stats(&stats, options, output, interactive)?;
    Ok(())
}

/// On exit, write the stats report if `--stats-out` was set and show a one-line
/// session summary on an interactive terminal.
fn finalize_stats(
    stats: &Stats,
    options: &DisplayOptions,
    output: &mut Output,
    interactive: bool,
) -> Result<()> {
    if let Some(path) = &options.stats_out {
        stats.write_report(path)?;
        output.notice(&format!(
            "[INFO] wrote activity stats to: {}",
            path.display()
        ))?;
    }

    if interactive && !stats.is_empty() {
        let files = stats.file_totals().len();
        output.notice(&format!(
            "-- session: {} lines in {} buckets across {} file(s) --",
            stats.total_lines(),
            stats.buckets.len(),
            files,
        ))?;
    }

    Ok(())
}

fn read_from_stdin(tx: Sender<LogMessage>) -> Result<()> {
    let stdin = io::stdin();
    let reader = stdin.lock();
    let mut current_tail_path: Option<PathBuf> = None;

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();

        // Parse GNU/BSD tail multi-file header format: "==> /path/to/file.log <=="
        if let Some(path) = trimmed
            .strip_prefix("==> ")
            .and_then(|s| s.strip_suffix(" <=="))
            .map(PathBuf::from)
        {
            current_tail_path = Some(path);
            continue;
        }

        // Parse line format: "filename: log content" or just "log content"
        // Try to extract filename if present
        let (path, content) = if let Some(colon_pos) = line.find(':') {
            let (potential_path, rest) = line.split_at(colon_pos);

            // Check if the part before colon looks like a path
            if potential_path.contains('/')
                || potential_path.contains('\\')
                || potential_path.ends_with(".log")
            {
                (
                    PathBuf::from(potential_path),
                    rest[1..].trim_start().to_string(),
                )
            } else if let Some(tail_path) = &current_tail_path {
                (tail_path.clone(), line.clone())
            } else {
                (PathBuf::from("stdin"), line.clone())
            }
        } else if let Some(tail_path) = &current_tail_path {
            (tail_path.clone(), line.clone())
        } else {
            (PathBuf::from("stdin"), line.clone())
        };

        if !content.is_empty() {
            tx.send(LogMessage::Line {
                path,
                line: content,
                timestamp: Local::now(),
            })?;
        }
    }

    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();

    let mut before_context = args.before_context;
    let mut after_context = args.after_context;

    if let Some(context) = args.context {
        if before_context == 0 {
            before_context = context;
        }
        if after_context == 0 {
            after_context = context;
        }
    }

    let use_color = !args.no_color && io::stdout().is_terminal();

    // Load config if specified
    let config = if let Some(config_path) = &args.config {
        Config::load(config_path)?
    } else {
        Config::default()
    };

    // `-P/--full-paths` overrides whatever the config file asks for.
    let path_style = if args.full_paths {
        PathStyle::Full
    } else {
        PathStyle::parse(&config.display.path_style)?
    };

    // CLI wins over the config file for both of these.
    let history_limit = args
        .history
        .or(config.history.limit)
        .unwrap_or(DEFAULT_HISTORY_LIMIT);

    let output_path = args.output.or_else(|| config.output.file.clone());
    let append = args.append || config.output.append;

    let stats_interval = args
        .stats_interval
        .or(config.stats.interval)
        .unwrap_or(DEFAULT_STATS_INTERVAL_SECS);
    if stats_interval < 1 {
        anyhow::bail!("--stats-interval must be at least 1 second (got {stats_interval})");
    }
    let stats_out = args.stats_out.or_else(|| config.stats.out.clone());

    let display_options = DisplayOptions {
        path_style,
        before_context,
        after_context,
        use_color,
        history_limit,
        output: output_path,
        append,
        stats_interval,
        stats_out,
    };

    // Handle stdin mode
    if args.stdin {
        // In stdin mode, we don't need directories - paths come from the stream.
        // Merge filters
        let mut include = args.include;
        include.extend(config.filters.include);

        let mut exclude = args.exclude;
        exclude.extend(config.filters.exclude);

        let filter = LogFilter::new(include, exclude)?;

        let (tx, rx) = channel();

        // Spawn stdin reader thread
        std::thread::spawn(move || {
            if let Err(e) = read_from_stdin(tx) {
                eprintln!("Stdin reader error: {}", e);
            }
        });

        // Use empty directories for stdin mode (paths come from stdin)
        display_logs(rx, filter, &[], display_options)?;

        return Ok(());
    }

    // Regular file watching mode
    let directories = if !args.directory.is_empty() {
        args.directory
    } else {
        config.watch_directories()
    };

    if directories.is_empty() {
        anyhow::bail!("No directories specified. Use -d or provide a config file.");
    }

    // Fail fast with a clear message rather than letting the watcher thread die
    // and leaving the user staring at an empty stream.
    for dir in &directories {
        if !dir.exists() {
            anyhow::bail!("Watch directory does not exist: {}", dir.display());
        }
        if !dir.is_dir() {
            anyhow::bail!("Watch path is not a directory: {}", dir.display());
        }
    }

    let file_pattern = if args.file_pattern != "*.log" {
        args.file_pattern
    } else {
        config.file_pattern().unwrap_or_else(|| "*.log".to_string())
    };

    // Merge filters
    let mut include = args.include;
    include.extend(config.filters.include);

    let mut exclude = args.exclude;
    exclude.extend(config.filters.exclude);

    let filter = LogFilter::new(include, exclude)?;

    let (tx, rx) = channel();

    let watch_dirs = directories.clone();
    let watch_pattern = file_pattern.clone();
    std::thread::spawn(move || {
        if let Err(e) = watch_files(watch_dirs, watch_pattern, tx) {
            eprintln!("Watcher error: {}", e);
        }
    });

    display_logs(rx, filter, &directories, display_options)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    /// Stands in for stdout so tests can assert on what reached the terminal.
    #[derive(Clone, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    impl SharedBuffer {
        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Per-test scratch directory, keyed by name so parallel tests don't collide.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("logwatch-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn roots(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    fn history_of(lines: &[&str], limit: usize) -> History {
        let mut history = History::new(limit);
        for line in lines {
            history.push(line, LogLevel::detect(line));
        }
        history
    }

    #[test]
    fn path_style_parses_documented_values() {
        assert_eq!(PathStyle::parse("full").unwrap(), PathStyle::Full);
        assert_eq!(PathStyle::parse("relative").unwrap(), PathStyle::Relative);
        assert_eq!(PathStyle::parse("filename").unwrap(), PathStyle::Filename);
    }

    #[test]
    fn path_style_rejects_unknown_values() {
        assert!(PathStyle::parse("basename").is_err());
    }

    #[test]
    fn path_style_renders_each_variant() {
        let path = Path::new("/var/log/app/error.log");
        let watch_roots = roots(&["/var/log"]);

        assert_eq!(
            PathStyle::Full.render(path, &watch_roots),
            "/var/log/app/error.log"
        );
        assert_eq!(
            PathStyle::Relative.render(path, &watch_roots),
            "app/error.log"
        );
        assert_eq!(PathStyle::Filename.render(path, &watch_roots), "error.log");
    }

    #[test]
    fn relative_falls_back_to_full_path_when_no_root_matches() {
        let path = Path::new("/opt/other/error.log");
        assert_eq!(
            PathStyle::Relative.render(path, &roots(&["/var/log"])),
            "/opt/other/error.log"
        );
    }

    #[test]
    fn relative_uses_the_first_matching_root() {
        let path = Path::new("/var/log/app/error.log");
        assert_eq!(
            PathStyle::Relative.render(path, &roots(&["/var/log", "/var/log/app"])),
            "app/error.log"
        );
    }

    #[test]
    fn filter_requires_an_include_match_and_no_exclude_match() {
        let filter = LogFilter::new(vec!["ERROR".into()], vec!["expected".into()]).unwrap();

        assert!(filter.matches("ERROR: database is down"));
        assert!(!filter.matches("INFO: all good"));
        assert!(!filter.matches("ERROR: expected failure in test"));
    }

    #[test]
    fn filter_without_includes_passes_everything_not_excluded() {
        let filter = LogFilter::new(vec![], vec!["DEBUG".into()]).unwrap();

        assert!(filter.matches("INFO: all good"));
        assert!(!filter.matches("DEBUG: noisy"));
    }

    #[test]
    fn filter_rejects_invalid_regex() {
        assert!(LogFilter::new(vec!["[unclosed".into()], vec![]).is_err());
    }

    #[test]
    fn log_level_detects_severities() {
        assert_eq!(LogLevel::detect("service PANIC unwound"), LogLevel::Fatal);
        assert_eq!(LogLevel::detect("ERROR: db down"), LogLevel::Error);
        assert_eq!(LogLevel::detect("WARNING: slow"), LogLevel::Warn);
        assert_eq!(LogLevel::detect("INFO: started"), LogLevel::Info);
        assert_eq!(LogLevel::detect("TRACE: entering"), LogLevel::Debug);
        assert_eq!(LogLevel::detect("plain message"), LogLevel::Other);
    }

    #[test]
    fn log_level_detection_is_case_insensitive() {
        assert_eq!(LogLevel::detect("error: lowercase"), LogLevel::Error);
    }

    #[test]
    fn history_evicts_oldest_lines_past_the_limit() {
        let history = history_of(&["one", "two", "three"], 2);

        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.entries[0].rendered, "two");
        assert_eq!(history.entries[1].rendered, "three");
    }

    #[test]
    fn history_of_zero_retains_nothing() {
        let history = history_of(&["one", "two"], 0);

        assert!(!history.is_enabled());
        assert!(history.entries.is_empty());
    }

    #[test]
    fn history_search_matches_message_and_path() {
        let history = history_of(
            &[
                "[t] app/server.log: ERROR: connection timeout",
                "[t] api/requests.log: INFO: ok",
                "[t] app/server.log: WARN: retrying",
            ],
            10,
        );

        let by_message = history.search(&Regex::new("timeout").unwrap());
        assert_eq!(by_message.len(), 1);

        // The rendered line is searched, so the file path is matchable too.
        let by_path = history.search(&Regex::new("app/server").unwrap());
        assert_eq!(by_path.len(), 2);
    }

    #[test]
    fn history_retains_lines_the_filter_would_hide() {
        // The point of retaining everything: search finds what the filter dropped.
        let filter = LogFilter::new(vec!["ERROR".into()], vec![]).unwrap();
        let mut history = History::new(10);

        for line in ["ERROR: db down", "WARN: disk almost full"] {
            history.push(line, LogLevel::detect(line));
        }

        assert!(!filter.matches("WARN: disk almost full"));
        assert_eq!(history.search(&Regex::new("disk").unwrap()).len(), 1);
    }

    #[test]
    fn history_search_preserves_detected_level() {
        let history = history_of(&["ERROR: db down"], 10);
        let matches = history.search(&Regex::new("db").unwrap());

        assert_eq!(matches[0].level, LogLevel::Error);
    }

    #[test]
    fn output_writes_plain_lines_to_the_export_file() {
        let dir = temp_dir("export");
        let path = dir.join("export.log");
        let term = SharedBuffer::default();

        {
            let mut output = Output::new(Box::new(term.clone()), Some(&path), false, true).unwrap();
            output
                .log("[t] app.log: ERROR: boom", LogLevel::Error)
                .unwrap();
            // Chrome and search replays must never reach the export file.
            output.notice("[INFO] Now watching: app.log").unwrap();
            output
                .echo("[t] app.log: ERROR: boom", LogLevel::Error)
                .unwrap();
            output.flush().unwrap();
        }

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, "[t] app.log: ERROR: boom\n");
        assert!(
            !written.contains('\u{1b}'),
            "export file must not contain ANSI color"
        );
        // All three writes still reached the terminal.
        assert_eq!(term.contents().lines().count(), 3);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn output_append_preserves_existing_content() {
        let dir = temp_dir("append");
        let path = dir.join("export.log");
        std::fs::write(&path, "earlier run\n").unwrap();

        {
            let mut output =
                Output::new(Box::new(SharedBuffer::default()), Some(&path), true, false).unwrap();
            output.log("later run", LogLevel::Other).unwrap();
            output.flush().unwrap();
        }

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "earlier run\nlater run\n"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn output_without_append_truncates() {
        let dir = temp_dir("truncate");
        let path = dir.join("export.log");
        std::fs::write(&path, "earlier run\n").unwrap();

        {
            let mut output =
                Output::new(Box::new(SharedBuffer::default()), Some(&path), false, false).unwrap();
            output.log("later run", LogLevel::Other).unwrap();
            output.flush().unwrap();
        }

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "later run\n");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn raw_mode_terminates_lines_with_carriage_return() {
        let term = SharedBuffer::default();
        let mut output = Output::new(Box::new(term.clone()), None, false, false).unwrap();

        output.notice("cooked").unwrap();
        output.raw_mode = true;
        output.notice("raw").unwrap();

        assert_eq!(term.contents(), "cooked\nraw\r\n");
    }

    #[test]
    fn color_is_applied_only_when_enabled() {
        let colored = SharedBuffer::default();
        let mut output = Output::new(Box::new(colored.clone()), None, false, true).unwrap();
        output.echo("ERROR: boom", LogLevel::Error).unwrap();
        assert!(colored.contents().contains('\u{1b}'));

        let plain = SharedBuffer::default();
        let mut output = Output::new(Box::new(plain.clone()), None, false, false).unwrap();
        output.echo("ERROR: boom", LogLevel::Error).unwrap();
        assert_eq!(plain.contents(), "ERROR: boom\n");
    }

    #[test]
    fn unclassified_lines_are_never_colored() {
        let term = SharedBuffer::default();
        let mut output = Output::new(Box::new(term.clone()), None, false, true).unwrap();

        output.echo("plain message", LogLevel::Other).unwrap();

        assert_eq!(term.contents(), "plain message\n");
    }

    // --- stats ---------------------------------------------------------------

    fn ts(hms: &str) -> DateTime<Local> {
        // Parse "YYYY-MM-DD HH:MM:SS" as a local timestamp for deterministic
        // bucketing in tests.
        let naive = chrono::NaiveDateTime::parse_from_str(hms, "%Y-%m-%d %H:%M:%S")
            .expect("valid datetime");
        Local
            .from_local_datetime(&naive)
            .single()
            .expect("unambiguous")
    }

    #[test]
    fn stats_buckets_by_interval() {
        let mut stats = Stats::new(60, STATS_MAX_BUCKETS);
        // Two lines in the same minute, one in the next.
        stats.record(ts("2026-07-23 14:16:05"), LogLevel::Error, "app.log");
        stats.record(ts("2026-07-23 14:16:59"), LogLevel::Warn, "app.log");
        stats.record(ts("2026-07-23 14:17:00"), LogLevel::Info, "app.log");

        assert_eq!(stats.buckets.len(), 2);
        assert_eq!(stats.total_lines(), 3);
    }

    #[test]
    fn stats_interval_changes_bucket_granularity() {
        let mut stats = Stats::new(300, STATS_MAX_BUCKETS); // 5-minute buckets
        stats.record(ts("2026-07-23 14:16:05"), LogLevel::Error, "app.log");
        stats.record(ts("2026-07-23 14:17:00"), LogLevel::Info, "app.log");
        // Both fall in the same 5-minute window.
        assert_eq!(stats.buckets.len(), 1);
    }

    #[test]
    fn stats_track_levels_and_files_jointly() {
        let mut stats = Stats::new(60, STATS_MAX_BUCKETS);
        stats.record(ts("2026-07-23 14:16:05"), LogLevel::Error, "app.log");
        stats.record(ts("2026-07-23 14:16:06"), LogLevel::Error, "app.log");
        stats.record(ts("2026-07-23 14:16:07"), LogLevel::Warn, "db.log");

        let levels = stats.level_totals();
        assert_eq!(levels[LogLevel::Error.index()], 2);
        assert_eq!(levels[LogLevel::Warn.index()], 1);

        let files = stats.file_totals();
        // Highest-volume file first.
        assert_eq!(files[0], ("app.log".to_string(), 2));
        assert_eq!(files[1], ("db.log".to_string(), 1));
    }

    #[test]
    fn stats_evict_oldest_bucket_past_the_cap() {
        let mut stats = Stats::new(60, 2);
        stats.record(ts("2026-07-23 14:16:00"), LogLevel::Info, "a.log");
        stats.record(ts("2026-07-23 14:17:00"), LogLevel::Info, "a.log");
        stats.record(ts("2026-07-23 14:18:00"), LogLevel::Info, "a.log");

        assert_eq!(stats.buckets.len(), 2);
        // The 14:16 bucket was dropped; the two most recent survive.
        let first = *stats.buckets.keys().next().unwrap();
        assert_eq!(
            Local
                .timestamp_opt(first, 0)
                .single()
                .unwrap()
                .format("%H:%M")
                .to_string(),
            "14:17"
        );
    }

    #[test]
    fn stats_fatal_and_error_share_the_error_bar_segment() {
        let mut bucket = StatsBucket::default();
        bucket.record(LogLevel::Fatal, "a.log");
        bucket.record(LogLevel::Error, "a.log");
        let stats = Stats::new(60, STATS_MAX_BUCKETS);
        // Whole bucket is error/fatal, so the bar is all '▓', no other shades.
        let bar = stats.bar(&bucket, 2);
        assert!(bar.chars().all(|c| c == '▓'), "bar was {bar:?}");
    }

    #[test]
    fn stats_csv_is_tidy_long_format() {
        let mut stats = Stats::new(60, STATS_MAX_BUCKETS);
        stats.record(ts("2026-07-23 14:16:05"), LogLevel::Error, "app.log");
        stats.record(ts("2026-07-23 14:16:06"), LogLevel::Warn, "app.log");

        let csv = stats.to_csv();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "bucket_start,file,level,count");
        // One row per (bucket, file, level) with a nonzero count.
        assert!(lines.iter().any(|l| l.contains("app.log,error,1")));
        assert!(lines.iter().any(|l| l.contains("app.log,warn,1")));
        // Zero-count levels are omitted.
        assert!(!lines.iter().any(|l| l.contains(",info,")));
    }

    #[test]
    fn stats_csv_quotes_fields_with_commas() {
        let mut stats = Stats::new(60, STATS_MAX_BUCKETS);
        stats.record(ts("2026-07-23 14:16:05"), LogLevel::Info, "weird,name.log");
        let csv = stats.to_csv();
        assert!(csv.contains("\"weird,name.log\""), "csv was:\n{csv}");
    }

    #[test]
    fn stats_json_round_trips_to_expected_shape() {
        let mut stats = Stats::new(120, STATS_MAX_BUCKETS);
        stats.record(ts("2026-07-23 14:16:05"), LogLevel::Error, "app.log");

        let json: serde_json::Value = serde_json::from_str(&stats.to_json().unwrap()).unwrap();
        assert_eq!(json["interval_seconds"], 120);
        assert_eq!(json["total_lines"], 1);
        let bucket = &json["buckets"][0];
        assert_eq!(bucket["total"], 1);
        assert_eq!(bucket["by_level"]["error"], 1);
        assert_eq!(bucket["by_file"]["app.log"]["error"], 1);
    }

    #[test]
    fn stats_report_by_extension_writes_json_or_csv() {
        let dir = temp_dir("stats");
        let mut stats = Stats::new(60, STATS_MAX_BUCKETS);
        stats.record(ts("2026-07-23 14:16:05"), LogLevel::Error, "app.log");

        let json_path = dir.join("report.json");
        stats.write_report(&json_path).unwrap();
        assert!(std::fs::read_to_string(&json_path)
            .unwrap()
            .contains("\"interval_seconds\""));

        let csv_path = dir.join("report.csv");
        stats.write_report(&csv_path).unwrap();
        assert!(std::fs::read_to_string(&csv_path)
            .unwrap()
            .starts_with("bucket_start,file,level,count"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stats_render_report_is_empty_message_when_no_activity() {
        let stats = Stats::new(60, STATS_MAX_BUCKETS);
        assert_eq!(
            stats.render_report(),
            vec!["-- no activity recorded yet --".to_string()]
        );
    }

    #[test]
    fn stats_render_report_includes_histogram_and_summary() {
        let mut stats = Stats::new(60, STATS_MAX_BUCKETS);
        stats.record(ts("2026-07-23 14:16:05"), LogLevel::Error, "app.log");
        stats.record(ts("2026-07-23 14:16:06"), LogLevel::Warn, "db.log");

        let report = stats.render_report().join("\n");
        assert!(report.contains("Activity"));
        assert!(report.contains("14:16"));
        assert!(report.contains("Totals: 2 lines"));
        assert!(report.contains("By file:"));
        assert!(report.contains("app.log"));
    }

    #[test]
    fn truncate_end_keeps_the_tail() {
        assert_eq!(truncate_end("short", 10), "short");
        assert_eq!(truncate_end("averylongfilename.log", 8), "…ame.log");
    }
}
