# Remote Log Streaming Guide

This guide shows you how to use logwatch's `--stdin` mode to stream and format logs from remote servers, containers, and other sources.

> **Always pass `ssh -n` when piping into logwatch.** Without it, ssh keeps
> reading your local keyboard and forwards it to the remote command, so your
> interactive keys (`q`, `p`, `s`, `/`) get swallowed before logwatch sees them.
> `-n` tells ssh not to read stdin, which is what you want when you're only
> streaming output. This matters most on Windows, where the keys stop working
> entirely without it. See [Troubleshooting](#interactive-keys-qps-dont-work-while-streaming).

## SSH Log Streaming

### Basic Remote Tailing

Stream logs from a remote server with nice formatting:

```bash
ssh -n user@production-server "tail -f /var/log/app/*.log" | logwatch --stdin
```

### With Filtering

Only show errors and warnings from remote server:

```bash
ssh -n user@server "tail -f /var/log/*.log" | logwatch --stdin -i "ERROR|WARN"
```

### Multiple Log Files

Tail multiple remote logs and combine them:

```bash
ssh -n user@server "tail -f /var/log/nginx/access.log /var/log/app/*.log" | logwatch --stdin
```

### Recursive Directory Streaming (All .log Files)

`tail -f` is not recursive by itself. Use `find` + `xargs` + `tail -F` to stream all log files under a directory tree:

```bash
ssh -n user@server "while true; do find /var/log/app -type f -name '*.log' -print0 | xargs -0 -r tail -n0 -F 2>/dev/null; sleep 1; done" | logwatch --stdin -P
```

This approach:
- Recursively discovers files under `/var/log/app`
- Follows rotations/recreates for tracked files (`-F`)
- Re-runs discovery so newly created `.log` files are picked up
- Works with `-P/--full-paths` because `tail` emits `==> /path <==` headers

### With Full Paths

Show the full file paths in the output:

```bash
ssh -n user@server "tail -f /var/log/app/*.log" | logwatch --stdin --full-paths
```

## Docker Container Logs

### Single Container

```bash
docker logs -f container_name | logwatch --stdin -i "ERROR|WARN"
```

### Docker Compose Service

```bash
docker compose logs -f web | logwatch --stdin
```

### Filter Out Health Checks

```bash
docker logs -f nginx | logwatch --stdin -e "health"
```

## Kubernetes Logs

### Pod Logs

```bash
kubectl logs -f pod-name | logwatch --stdin -i "error|exception"
```

### Multiple Containers in Pod

```bash
kubectl logs -f pod-name -c container-name | logwatch --stdin
```

### Deployment Logs

```bash
kubectl logs -f deployment/my-app | logwatch --stdin
```

## Systemd Journal

### Service Logs

```bash
journalctl -f -u myservice.service | logwatch --stdin
```

### Filter by Priority

```bash
journalctl -f -p err | logwatch --stdin
```

### Since Boot

```bash
journalctl -f -b | logwatch --stdin -i "error|failed"
```

## Advanced Patterns

### Multiple Remote Servers

Use SSH multiplexing to monitor multiple servers:

```bash
# Terminal 1
ssh -n server1 "tail -f /var/log/*.log" | logwatch --stdin &

# Terminal 2  
ssh -n server2 "tail -f /var/log/*.log" | logwatch --stdin
```

Or combine them:

```bash
(ssh -n server1 "tail -f /var/log/*.log"; ssh -n server2 "tail -f /var/log/*.log") | logwatch --stdin
```

### Remote + Local Logs

Run two instances to watch both:

```bash
# Terminal 1: Local logs
logwatch -d ./logs

# Terminal 2: Remote logs
ssh -n server "tail -f /var/log/*.log" | logwatch --stdin
```

### Persistent SSH Connection

Use SSH ControlMaster for faster connections:

```bash
# In ~/.ssh/config
Host production
    HostName prod.example.com
    User deploy
    ControlMaster auto
    ControlPath ~/.ssh/cm-%r@%h:%p
    ControlPersist 10m

# Then use it
ssh -n production "tail -f /var/log/*.log" | logwatch --stdin
```

## Understanding Stdin Input Format

Logwatch's stdin mode tries to parse the input intelligently:

### Format 1: With File Path Prefix

```
/var/log/app.log: ERROR: Connection failed
api/server.log: WARN: Slow response
```

Logwatch will extract the path and content.

### Format 2: Plain Log Lines

```
ERROR: Connection failed
WARN: Slow response
```

Logwatch will label these as `stdin` source.

### Format 3: Tail Output

When using `tail -f`, the default output format works perfectly:

```bash
==> /var/log/app.log <==
ERROR: Connection failed

==> /var/log/api.log <==
WARN: Slow response
```

## Password Authentication with sshpass

If SSH key authentication is not available on the remote server, you can use `sshpass` to provide the password non-interactively.

### Install sshpass

```bash
# macOS
brew install sshpass

# Debian/Ubuntu
sudo apt install sshpass
```

### Store the Password Securely

Avoid putting passwords directly in commands (they'll appear in shell history). Use a password file instead:

```bash
echo 'yourpassword' > ~/.ssh_pass && chmod 600 ~/.ssh_pass
```

### Basic Usage

```bash
sshpass -f ~/.ssh_pass ssh -n user@server "tail -f /var/log/*.log" | logwatch --stdin
```

### Recursive Directory Streaming with sshpass

```bash
sshpass -f ~/.ssh_pass ssh -n user@server \
  "while true; do find /var/log/app -type f -name '*.log' -print0 | xargs -0 -r tail -n0 -F 2>/dev/null; sleep 1; done" \
  | logwatch --stdin -P --no-color
```

## Saving Output to a File

### File Only (No Terminal Output)

```bash
ssh -n user@server "tail -f /var/log/*.log" | logwatch --stdin --no-color > output.log 2>&1
```

### Live View and File Simultaneously

Use `tee` to see the output in your terminal while also saving to a file:

```bash
ssh -n user@server "tail -f /var/log/*.log" | logwatch --stdin --no-color | tee output.log
```

To append to an existing file instead of overwriting:

```bash
ssh -n user@server "tail -f /var/log/*.log" | logwatch --stdin --no-color | tee -a output.log
```

### Combining sshpass with tee

```bash
sshpass -f ~/.ssh_pass ssh -n user@server \
  "while true; do find /var/log/app -type f -name '*.log' -print0 | xargs -0 -r tail -n0 -F 2>/dev/null; sleep 1; done" \
  | logwatch --stdin -P --no-color | tee raw_logs.txt
```

### Save Only Filtered Lines

```bash
ssh -n user@server "tail -f /var/log/*.log" | \
  logwatch --stdin --no-color -i "ERROR|WARN" | \
  tee filtered.log
```

> **Note:** Always use `--no-color` when saving to a file to prevent ANSI escape codes from polluting the output.

## Tips and Tricks

### Keep SSH Alive

Add to your SSH command to keep connection alive:

```bash
ssh -n -o ServerAliveInterval=60 user@server "tail -f /var/log/*.log" | logwatch --stdin
```

### Run in Background

```bash
ssh -n user@server "tail -f /var/log/*.log" | logwatch --stdin --no-color > output.log 2>&1 &
```

### Reconnect on Disconnect

Use a loop to auto-reconnect:

```bash
while true; do
  ssh -n user@server "tail -f /var/log/*.log" | logwatch --stdin
  echo "Connection lost, reconnecting in 5 seconds..."
  sleep 5
done
```

### BSD/macOS Remote Variant (No `xargs -r`)

Some systems don't support `xargs -r`. Use this variant:

```bash
ssh -n user@server "while true; do files=\$(find /var/log/app -type f -name '*.log'); [ -n \"\$files\" ] && tail -n0 -F \$files; sleep 1; done" | logwatch --stdin -P
```

### Save Filtered Output

See [Saving Output to a File](#saving-output-to-a-file) for full details on saving to files with or without live terminal output.

## Common Issues

### Interactive keys (q/p/s) don't work while streaming

By default `ssh` reads your local keyboard and forwards it to the remote
command's stdin. When you pipe `ssh … | logwatch --stdin`, that means your
keypresses go to the remote `tail` (which ignores them) instead of to logwatch,
so `q`, `p`, `s`, and `/` appear dead.

Fix: add **`-n`** so ssh stops reading stdin — you're only streaming output back,
not sending anything to the remote:

```bash
ssh -n user@server "tail -f /var/log/*.log" | logwatch --stdin
```

Notes:
- On **Windows** the keys stop working *entirely* without `-n` (the console
  input is drained by ssh). On **macOS/Linux** logwatch reads the controlling
  terminal directly, so it usually still gets the keys, but a stray first press
  can be swallowed — `-n` makes it reliable everywhere.
- With `sshpass`, put `-n` on the `ssh` part: `sshpass -f pass ssh -n user@server …`.
- Make sure the `|` is *outside* the ssh quotes, so logwatch runs on your local
  machine: `ssh -n host "tail -f …" | logwatch --stdin` — not
  `ssh -n host "tail -f … | logwatch --stdin"`.

### "Connection refused"

Make sure you can SSH to the server normally first:
```bash
ssh -n user@server echo "test"
```

### No Output Appearing

Check that the remote command works directly:
```bash
ssh -n user@server "tail -n 10 /var/log/*.log"
```

If you're using recursive streaming, verify discovery returns files:
```bash
ssh -n user@server "find /var/log/app -type f -name '*.log' | head"
```

### Filtering Not Working

Test your regex patterns locally first:
```bash
echo "ERROR: test" | logwatch --stdin -i "ERROR"
```

### SSH Times Out

Add these options:
```bash
ssh -n -o ServerAliveInterval=30 -o ServerAliveCountMax=3 user@server "tail -f /var/log/*.log" | logwatch --stdin
```

### `xargs: tail: terminated by signal 13`

This is usually a broken pipe (`SIGPIPE`) when the downstream reader exits (for example, `logwatch` stopped). It's generally harmless on shutdown.

Use stderr suppression on the remote `tail` if needed:
```bash
ssh -n user@server "while true; do find /var/log/app -type f -name '*.log' -print0 | xargs -0 -r tail -n0 -F 2>/dev/null; sleep 1; done" | logwatch --stdin
```

## Performance Considerations

- **Latency**: There will be some latency from SSH + pipe overhead (usually <1 second)
- **Bandwidth**: Text logs are small, but high-volume logs may slow down
- **SSH compression**: Add `-C` flag to SSH for compression if bandwidth is limited

```bash
ssh -n -C user@server "tail -f /var/log/*.log" | logwatch --stdin
```

## Security Notes

- Prefer SSH keys over passwords for automated streaming (see [sshpass](#password-authentication-with-sshpass) if key auth is unavailable)
- Be careful with `-P/--full-paths` as it may expose server file structure
- Consider using SSH tunneling for production environments
- Logs may contain sensitive data - don't pipe to untrusted systems

## Comparison with Other Tools

### vs. Plain `tail -f`
- ✅ Better formatting with timestamps
- ✅ Powerful regex filtering
- ✅ Interactive controls (pause/resume/clear)

### vs. `lnav`
- ✅ Works over SSH without remote installation
- ✅ Lighter weight
- ❌ Less advanced features (no SQL queries)

### vs. Log aggregation services (Splunk, ELK)
- ✅ Zero setup required
- ✅ Works anywhere you have SSH
- ❌ Not for long-term storage or complex queries

## Example Workflows

### Debug Production Issue

```bash
# Stream production logs, filter for errors
ssh -n prod "tail -f /var/log/app/*.log" | logwatch --stdin -i "ERROR|EXCEPTION"

# Press 'p' to pause when you see the issue
# Press 'c' to clear and continue monitoring
```

### Monitor Deployment

```bash
# Watch application start up
kubectl logs -f deployment/my-app | logwatch --stdin -e "health_check"
```

### Compare Two Environments

```bash
# Terminal 1: Staging
ssh -n staging "tail -f /var/log/app/*.log" | logwatch --stdin -i "ERROR"

# Terminal 2: Production
ssh -n prod "tail -f /var/log/app/*.log" | logwatch --stdin -i "ERROR"
```
