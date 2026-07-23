# LogWatch

[![CI](https://github.com/shapedthought/logwatch/actions/workflows/ci.yml/badge.svg)](https://github.com/shapedthought/logwatch/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

A fast, real-time log file monitoring tool written in Rust. Watch multiple directories recursively, filter log output, and see everything in one unified stream.

## Features

- 🔍 **Recursive directory watching** - Monitor entire directory trees for log files
- 🔄 **Real-time streaming** - See new log lines as they're written
- 🎯 **Powerful filtering** - Include/exclude patterns using regex
- 🎨 **Color-coded levels** - Highlights ERROR/WARN/INFO/DEBUG lines in terminal output
- 📚 **Match context output** - Show lines before/after matches with `-A/-B/-C`
- 📁 **Multiple file patterns** - Watch `*.log`, `*.txt`, or custom patterns
- ⚙️ **Flexible configuration** - Use CLI args, config files, or both
- 🖥️ **Interactive TUI** - Pause, resume, and clear output on the fly
- 🔎 **Searchable history** - Press `/` to search back over everything seen, including lines your filters hid
- 📊 **Activity stats** - Press `s` for a timeline histogram of log volume by severity and file; export to CSV/JSON
- 💾 **Export to file** - Mirror the displayed stream to a plain-text file with `-o`
- 📍 **Smart path display** - Show full, relative, or just filename paths

## Installation

### Pre-built binaries

Download the archive for your platform from the
[latest release](https://github.com/shapedthought/logwatch/releases/latest),
extract it, and put `logwatch` somewhere on your `PATH`:

```bash
tar -xzf logwatch-<version>-<target>.tar.gz
sudo install logwatch-<version>-<target>/logwatch /usr/local/bin/
```

Binaries are published for Linux (x86_64/aarch64, gnu and musl), macOS
(Intel and Apple Silicon), and Windows (x86_64/aarch64). Each archive ships
with a `.sha256` checksum file.

### From source

Requires Rust 1.74 or newer.

```bash
git clone https://github.com/shapedthought/logwatch.git
cd logwatch
cargo build --release
```

The binary will be at `target/release/logwatch`

### Install globally

```bash
cargo install --path .
```

## Documentation

- [QUICKSTART.md](QUICKSTART.md) - get up and running in five minutes
- [REMOTE_STREAMING.md](REMOTE_STREAMING.md) - SSH, Docker, and Kubernetes streaming guide
- [config.example.toml](config.example.toml) - annotated configuration reference

## Quick Start

### Watch a single directory

```bash
logwatch -d /var/log
```

### Watch multiple directories

```bash
logwatch -d /var/log -d ~/app/logs
```

### Filter for errors and warnings

```bash
logwatch -d ./logs -i "ERROR" -i "WARN"
```

### Exclude debug messages

```bash
logwatch -d ./logs -e "DEBUG"
```

### Disable color output

```bash
logwatch -d ./logs -i "ERROR|WARN" --no-color
```

### Show context around matches

```bash
# 5 lines before and after each match
logwatch -d ./logs -i "ERROR" -C 5

# 3 lines before, 1 line after
logwatch -d ./logs -i "ERROR" -B 3 -A 1
```

### Use full paths instead of relative

```bash
logwatch -d /var/log --full-paths
```

### Watch specific file patterns

```bash
logwatch -d ./logs -f "*.txt"
```

### Save the filtered output to a file

```bash
# Write every displayed line to errors.log as well as the terminal
logwatch -d ./logs -i "ERROR" -o errors.log

# Append to an existing file instead of overwriting it
logwatch -d ./logs -i "ERROR" -o errors.log --append
```

### Track activity over time

```bash
# Press 's' while running for a histogram; write a report on exit
logwatch -d ./logs --stats-out activity.csv

# Use 5-minute buckets and export JSON instead
logwatch -d ./logs --stats-interval 300 --stats-out activity.json
```

### Read from stdin (for remote log streaming)

```bash
# Stream logs from remote server via SSH
ssh user@server "tail -f /var/log/app/*.log" | logwatch --stdin

# Recursively stream all .log files (including newly created files)
ssh user@server "while true; do find /var/log/app -type f -name '*.log' -print0 | xargs -0 -r tail -n0 -F 2>/dev/null; sleep 1; done" | logwatch --stdin -P

# With filtering
ssh user@server "tail -f /var/log/*.log" | logwatch --stdin -i "ERROR|WARN"

# With filtering + context
ssh user@server "tail -f /var/log/*.log" | logwatch --stdin -i "ERROR|WARN" -C 3

# Stream from docker container
docker logs -f container_name | logwatch --stdin

# Stream from journalctl
journalctl -f | logwatch --stdin -i "error"
```

## Configuration File

Create a `logwatch.toml` file for persistent configuration:

```toml
[watch]
directories = [
    "/var/log",
    "./logs"
]
file_pattern = "*.log"

[display]
# "relative" (default), "full", or "filename"
path_style = "relative"

[filters]
include = ["ERROR", "WARN"]
exclude = ["DEBUG"]

[history]
limit = 10000

[output]
file = "logwatch.out"
append = false

[stats]
interval = 60          # time-bucket size in seconds
out = "activity.csv"   # report written at exit (.json for JSON, else CSV)
```

Then use it:

```bash
logwatch -c logwatch.toml
```

See `config.example.toml` for a complete example with comments.

## CLI Reference

```
Usage: logwatch [OPTIONS]

Options:
  -d, --directory <DIR>       Directory to watch (can be specified multiple times)
  -c, --config <FILE>         Configuration file path
  -P, --full-paths            Use full absolute paths instead of relative
  -i, --include <PATTERN>     Include pattern (regex, can be specified multiple times)
  -e, --exclude <PATTERN>     Exclude pattern (regex, can be specified multiple times)
  -B, --before-context <NUM>  Show NUM lines of leading context before each match [default: 0]
  -A, --after-context <NUM>   Show NUM lines of trailing context after each match [default: 0]
  -C, --context <NUM>         Show NUM lines of output context around each match
  -f, --file-pattern <PATTERN> File pattern to watch (e.g., "*.log") [default: *.log]
      --stdin                 Read from stdin instead of watching files (for remote streaming)
  -o, --output <FILE>         Also write displayed lines to FILE (plain text, no color)
      --append                Append to the output file instead of overwriting it
      --history <NUM>         Lines to retain for '/' search [default: 10000]
      --stats-interval <SECS> Time-bucket size for activity stats [default: 60]
      --stats-out <FILE>      Write an activity-stats report at exit (.json = JSON, else CSV)
      --no-color              Disable colored output
  -h, --help                  Print help
  -V, --version               Print version
```

`-P/--full-paths` is a shorthand for `path_style = "full"` and overrides
whatever the config file sets. Color is disabled automatically when stdout is
not a terminal, so `--no-color` is only needed to suppress it in a terminal.

## Interactive Controls

While logwatch is running:

- **`q`** (or `Ctrl-C`) - Quit the application
- **`c`** - Clear the screen
- **`p`** - Pause/resume log output. While paused, incoming lines are buffered rather than dropped
- **`/`** - Search the retained history (see below). `Enter` runs the search, `Esc` cancels
- **`s`** - Show the activity-stats histogram (see below)

Keys are read from the controlling terminal rather than stdin, so they keep
working in `--stdin` mode while a pipe is feeding logs in.

## History and Search

LogWatch keeps the last 10,000 lines in memory (`--history NUM` to change,
`--history 0` to disable). Press `/`, type a regex, and press `Enter` to replay
the matches inline:

```
/timeout
-- 2 matches for /timeout (8412 lines retained) --
[2026-03-14 10:28:02] app/server.log: ERROR: connection timeout
[2026-03-14 10:29:41] db/queries.log: WARN: query timeout after 30s
-- end of matches, streaming resumed --
```

Two things worth knowing:

- **History retains every line seen, including ones your filters hid.** If you
  started with `-i "ERROR"` and later need the `WARN` you skipped past, `/` will
  still find it - no need to restart with a different filter.
- **The whole rendered line is searched**, so the timestamp and file path are
  matchable too: `/server\.log` shows everything from that one file.

Searching does not disturb the stream - incoming lines queue up while you type
and resume immediately afterwards. A search replays at most 100 matches, newest
last, so a broad pattern cannot flood the terminal.

When an input stream ends (the remote `tail` died, the container stopped),
LogWatch stays open so the history is still searchable; press `q` to exit.

## Activity Stats

LogWatch buckets log activity by time so you can see *when* things happened and
correlate bursts with system operations. Press `s` for a live histogram:

```
── Activity ── 60s buckets ── 2026-03-14 10:12-10:18 ──
   (▓ error/fatal  ▒ warn  ░ info/debug/other)
10:12  ░░░                                 6
10:14  ▒▒░░░░░░░░░░░                       42
10:16  ▓▓▓▓▓▓▓▒▒▒░░░░░░░░░░░░░░░░░░░░░░░   198
10:18  ▓▒░░░                              14
Totals: 260 lines — error 61, warn 44, info 120, other 35
By file:
  app/server.log                       181  ████████████████████████
  api/requests.log                      52  ███████
  db/queries.log                        27  ███
```

- **`--stats-interval SECS`** sets the bucket size (default 60). Smaller buckets
  give finer resolution; larger ones smooth out the timeline.
- **`--stats-out FILE`** writes a full report when LogWatch exits. The format is
  chosen by extension: `.json` for JSON, anything else for CSV.

The CSV is tidy long-format — one row per time bucket, file, and severity — which
drops straight into a spreadsheet or plotting tool:

```csv
bucket_start,file,level,count
2026-03-14T10:16:00+00:00,app/server.log,error,54
2026-03-14T10:16:00+00:00,app/server.log,warn,18
2026-03-14T10:16:00+00:00,api/requests.log,info,42
```

A note on timing: buckets are keyed by **when LogWatch sees each line**, not by
any timestamp embedded in the log itself. For live monitoring that's the same
thing; it's not meant for reconstructing timelines from an old file after the
fact. Stats count every line seen, including ones your filters hid, so the
histogram reflects true activity rather than the filtered view.

## Examples

### Development scenario - watch application logs

```bash
logwatch -d ./logs -i "ERROR|WARN|FATAL"
```

### Production monitoring - multiple services

```bash
logwatch -d /var/log/nginx -d /var/log/app -e "health_check"
```

### Custom file patterns

```bash
# Watch all .txt and .log files
logwatch -d ./logs -f "*.{log,txt}"

# Watch error logs only
logwatch -d /var/log -f "*error*.log"
```

### Combining filters

```bash
# Show errors but exclude known issues
logwatch -d ./logs -i "ERROR" -e "connection_timeout" -e "expected_error"

# Show context around each error
logwatch -d ./logs -i "ERROR" -C 5
```

### Remote log streaming

```bash
# Monitor remote server logs with nice formatting
ssh production-server "tail -f /var/log/nginx/*.log" | logwatch --stdin

# Recursively follow all logs from a directory tree
ssh production-server "while true; do find /var/log/nginx -type f -name '*.log' -print0 | xargs -0 -r tail -n0 -F 2>/dev/null; sleep 1; done" | logwatch --stdin -P

# Multiple remote sources via SSH multiplexing
ssh server1 "tail -f /app/logs/*.log" | logwatch --stdin -i "ERROR|CRITICAL"

# Docker container logs
docker logs -f my-app | logwatch --stdin

# Kubernetes pod logs
kubectl logs -f pod-name | logwatch --stdin -i "ERROR"

# Stream systemd journal
journalctl -f -u myservice.service | logwatch --stdin
```

Note: `tail -f` with shell globs is not recursive by itself. The `find ... | xargs tail -F` loop is what discovers files recursively and re-discovers new files over time.

## How It Works

1. **Discovery** - LogWatch recursively scans specified directories for files matching the pattern
2. **Watching** - Uses the `notify` crate to monitor file system events
3. **Streaming** - Efficiently reads new content as it's written (like `tail -f` but for multiple files)
4. **Filtering** - Applies include/exclude regex patterns before display
5. **Display** - Shows unified output with timestamps and configurable path information

## Output Format

```
[2024-01-28 10:30:15] app/server.log: ERROR: Connection failed to database
[2024-01-28 10:30:16] api/requests.log: WARN: Slow response time: 5000ms
[2024-01-28 10:30:17] app/server.log: ERROR: Retry attempt failed
```

Each line shows:
- Timestamp (when the line was detected)
- File path (configurable: full, relative, or filename only)
- The actual log content

## Use Cases

- **Development** - Monitor application logs during development without jumping between files
- **Debugging** - Watch multiple log sources simultaneously to correlate issues
- **Production monitoring** - Real-time visibility into server logs
- **Remote log streaming** - SSH into servers and stream logs with better formatting than plain `tail -f`
- **Container/orchestration** - Format Docker and Kubernetes logs for better readability
- **Log aggregation** - Unified view when products don't provide log streaming

## Performance

LogWatch is designed to be lightweight and efficient:
- Only watches files that match patterns
- Reads only new content (no re-reading entire files)
- Minimal memory footprint
- Handles log rotation gracefully

## Future Enhancements (Roadmap)

- [ ] Desktop notifications for critical errors
- [ ] LLM integration for error analysis and suggestions
- [ ] Multiple output formats (JSON, CSV)

## Contributing

Contributions welcome! Feel free to open issues or submit pull requests.

Before opening a PR, please make sure the checks CI runs pass locally:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

## License

Licensed under the [MIT License](LICENSE).
