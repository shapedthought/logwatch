# Quick Start Guide

## Building the Project

1. Make sure you have Rust installed:
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. Build the project:
   ```bash
   cd logwatch
   cargo build --release
   ```

3. The binary will be at `target/release/logwatch`

## Testing It Out

### Option 1: Use the Test Log Generator

In one terminal, generate test logs:
```bash
# Linux / macOS
./generate_test_logs.sh

# Windows (PowerShell)
.\generate_test_logs.ps1
```

In another terminal, run logwatch:
```bash
cargo run -- -d ./test_logs
```

### Option 2: Watch Your Own Logs

Watch your system logs:
```bash
# Linux
cargo run -- -d /var/log

# macOS
cargo run -- -d /var/log

# Custom application logs
cargo run -- -d ~/projects/myapp/logs
```

## Common Usage Patterns

### Filter for errors only
```bash
cargo run -- -d ./logs -i "ERROR"
```

### Filter for errors and warnings
```bash
cargo run -- -d ./logs -i "ERROR|WARN"
```

### Exclude debug messages
```bash
cargo run -- -d ./logs -e "DEBUG"
```

### Watch multiple directories
```bash
cargo run -- -d /var/log -d ~/app/logs -d ./logs
```

### Use full paths
```bash
cargo run -- -d ./logs --full-paths
```

### Stream remote logs over SSH (stdin mode)
```bash
ssh user@server "tail -f /var/log/app/*.log" | cargo run -- --stdin
```

### Recursively stream all remote .log files
```bash
ssh user@server "while true; do find /var/log/app -type f -name '*.log' -print0 | xargs -0 -r tail -n0 -F 2>/dev/null; sleep 1; done" | cargo run -- --stdin -P
```

### Copy/paste template for an application log tree
```bash
# Local machine runs logwatch; remote server streams the log tree recursively
ssh user@server "while true; do find /var/log/myapp -type f -name '*.log' -print0 | xargs -0 -r tail -n0 -F 2>/dev/null; sleep 1; done" | logwatch --stdin -P

# Same, with filtering and 3 lines of context
ssh user@server "while true; do find /var/log/myapp -type f -name '*.log' -print0 | xargs -0 -r tail -n0 -F 2>/dev/null; sleep 1; done" | logwatch --stdin -P -i "ERROR|WARN|FAILED" -C 3
```

Notes:
- Plain `tail -f` globs are not recursive.
- The `find ... | xargs ... tail -F` loop re-discovers new files.
- Use `-P/--full-paths` to show full file paths when `tail` emits headers.

## Using a Configuration File

1. Copy the example config:
   ```bash
   cp config.example.toml my-config.toml
   ```

2. Edit it to match your needs:
   ```toml
   [watch]
   directories = ["./logs", "/var/log"]
   
   [filters]
   include = ["ERROR", "WARN"]
   ```

3. Run with config:
   ```bash
   cargo run -- -c my-config.toml
   ```

## Saving Output to a File

```bash
# Mirror the displayed lines into errors.log
cargo run -- -d ./logs -i "ERROR" -o errors.log

# Append instead of overwriting
cargo run -- -d ./logs -i "ERROR" -o errors.log --append
```

The file gets plain text with no color codes, so it stays greppable.

## Searching What You Have Already Seen

Press **`/`**, type a regex, press **Enter**:

```
/timeout
-- 2 matches for /timeout (412 lines retained) --
[2026-03-14 10:28:02] app/server.log: ERROR: connection timeout
[2026-03-14 10:29:41] db/queries.log: WARN: query timeout after 30s
-- end of matches, streaming resumed --
```

History keeps every line seen - even ones your `-i`/`-e` filters hid - so you
can go looking for something you filtered out without restarting. Use
`--history NUM` to change how much is retained, or `--history 0` to turn it off.

## Tracking Activity Over Time

Press **`s`** for a histogram of log volume over time, broken down by severity
and file — handy for spotting when a burst of errors lined up with a deploy or
a job run:

```bash
# Write a CSV report when you quit (open it in a spreadsheet)
cargo run -- -d ./logs --stats-out activity.csv

# 5-minute buckets, exported as JSON
cargo run -- -d ./logs --stats-interval 300 --stats-out activity.json
```

Buckets are keyed by when logwatch sees each line, and every line counts (even
ones your filters hid), so the histogram shows true activity.

## Interactive Controls

While running:
- Press **`q`** (or `Ctrl-C`) to quit
- Press **`c`** to clear the screen
- Press **`p`** to pause/resume (lines are buffered while paused, not dropped)
- Press **`/`** to search the history, **`Esc`** to cancel
- Press **`s`** to show the activity-stats histogram

## Installing Globally

To use `logwatch` from anywhere:

```bash
cargo install --path .
```

Then you can just run:
```bash
logwatch -d /var/log
```

## Troubleshooting

### "No such file or directory"
Make sure the directories you're watching exist and you have read permissions.

### No output appearing
- Check that the directory contains files matching the pattern (default: `*.log`)
- Try using `-f "*.txt"` if your logs have a different extension
- Verify files are being written to (use `ls -la` to check timestamps)

### Permission denied
You may need to run with `sudo` to access system logs:
```bash
sudo cargo run -- -d /var/log
```

## Next Steps

- Read the full [README.md](README.md) for all features
- Check out [config.example.toml](config.example.toml) for configuration options
- Try different filter patterns to find what works for your use case
