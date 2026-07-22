# Remote Log Streaming Guide

This guide shows you how to use logwatch's `--stdin` mode to stream and format logs from remote servers, containers, and other sources.

## SSH Log Streaming

### Basic Remote Tailing

Stream logs from a remote server with nice formatting:

```bash
ssh user@production-server "tail -f /var/log/app/*.log" | logwatch --stdin
```

### With Filtering

Only show errors and warnings from remote server:

```bash
ssh user@server "tail -f /var/log/*.log" | logwatch --stdin -i "ERROR|WARN"
```

### Multiple Log Files

Tail multiple remote logs and combine them:

```bash
ssh user@server "tail -f /var/log/nginx/access.log /var/log/app/*.log" | logwatch --stdin
```

### Recursive Directory Streaming (All .log Files)

`tail -f` is not recursive by itself. Use `find` + `xargs` + `tail -F` to stream all log files under a directory tree:

```bash
ssh user@server "while true; do find /var/log/app -type f -name '*.log' -print0 | xargs -0 -r tail -n0 -F 2>/dev/null; sleep 1; done" | logwatch --stdin -P
```

This approach:
- Recursively discovers files under `/var/log/app`
- Follows rotations/recreates for tracked files (`-F`)
- Re-runs discovery so newly created `.log` files are picked up
- Works with `-P/--full-paths` because `tail` emits `==> /path <==` headers

### With Full Paths

Show the full file paths in the output:

```bash
ssh user@server "tail -f /var/log/app/*.log" | logwatch --stdin --full-paths
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
ssh server1 "tail -f /var/log/*.log" | logwatch --stdin &

# Terminal 2  
ssh server2 "tail -f /var/log/*.log" | logwatch --stdin
```

Or combine them:

```bash
(ssh server1 "tail -f /var/log/*.log"; ssh server2 "tail -f /var/log/*.log") | logwatch --stdin
```

### Remote + Local Logs

Run two instances to watch both:

```bash
# Terminal 1: Local logs
logwatch -d ./logs

# Terminal 2: Remote logs
ssh server "tail -f /var/log/*.log" | logwatch --stdin
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
ssh production "tail -f /var/log/*.log" | logwatch --stdin
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
sshpass -f ~/.ssh_pass ssh user@server "tail -f /var/log/*.log" | logwatch --stdin
```

### Recursive Directory Streaming with sshpass

```bash
sshpass -f ~/.ssh_pass ssh user@server \
  "while true; do find /var/log/app -type f -name '*.log' -print0 | xargs -0 -r tail -n0 -F 2>/dev/null; sleep 1; done" \
  | logwatch --stdin -P --no-color
```

## Saving Output to a File

### File Only (No Terminal Output)

```bash
ssh user@server "tail -f /var/log/*.log" | logwatch --stdin --no-color > output.log 2>&1
```

### Live View and File Simultaneously

Use `tee` to see the output in your terminal while also saving to a file:

```bash
ssh user@server "tail -f /var/log/*.log" | logwatch --stdin --no-color | tee output.log
```

To append to an existing file instead of overwriting:

```bash
ssh user@server "tail -f /var/log/*.log" | logwatch --stdin --no-color | tee -a output.log
```

### Combining sshpass with tee

```bash
sshpass -f ~/.ssh_pass ssh user@server \
  "while true; do find /var/log/app -type f -name '*.log' -print0 | xargs -0 -r tail -n0 -F 2>/dev/null; sleep 1; done" \
  | logwatch --stdin -P --no-color | tee raw_logs.txt
```

### Save Only Filtered Lines

```bash
ssh user@server "tail -f /var/log/*.log" | \
  logwatch --stdin --no-color -i "ERROR|WARN" | \
  tee filtered.log
```

> **Note:** Always use `--no-color` when saving to a file to prevent ANSI escape codes from polluting the output.

## Tips and Tricks

### Keep SSH Alive

Add to your SSH command to keep connection alive:

```bash
ssh -o ServerAliveInterval=60 user@server "tail -f /var/log/*.log" | logwatch --stdin
```

### Run in Background

```bash
ssh user@server "tail -f /var/log/*.log" | logwatch --stdin --no-color > output.log 2>&1 &
```

### Reconnect on Disconnect

Use a loop to auto-reconnect:

```bash
while true; do
  ssh user@server "tail -f /var/log/*.log" | logwatch --stdin
  echo "Connection lost, reconnecting in 5 seconds..."
  sleep 5
done
```

### BSD/macOS Remote Variant (No `xargs -r`)

Some systems don't support `xargs -r`. Use this variant:

```bash
ssh user@server "while true; do files=\$(find /var/log/app -type f -name '*.log'); [ -n \"\$files\" ] && tail -n0 -F \$files; sleep 1; done" | logwatch --stdin -P
```

### Save Filtered Output

See [Saving Output to a File](#saving-output-to-a-file) for full details on saving to files with or without live terminal output.

## Common Issues

### "Connection refused"

Make sure you can SSH to the server normally first:
```bash
ssh user@server echo "test"
```

### No Output Appearing

Check that the remote command works directly:
```bash
ssh user@server "tail -n 10 /var/log/*.log"
```

If you're using recursive streaming, verify discovery returns files:
```bash
ssh user@server "find /var/log/app -type f -name '*.log' | head"
```

### Filtering Not Working

Test your regex patterns locally first:
```bash
echo "ERROR: test" | logwatch --stdin -i "ERROR"
```

### SSH Times Out

Add these options:
```bash
ssh -o ServerAliveInterval=30 -o ServerAliveCountMax=3 user@server "tail -f /var/log/*.log" | logwatch --stdin
```

### `xargs: tail: terminated by signal 13`

This is usually a broken pipe (`SIGPIPE`) when the downstream reader exits (for example, `logwatch` stopped). It's generally harmless on shutdown.

Use stderr suppression on the remote `tail` if needed:
```bash
ssh user@server "while true; do find /var/log/app -type f -name '*.log' -print0 | xargs -0 -r tail -n0 -F 2>/dev/null; sleep 1; done" | logwatch --stdin
```

## Performance Considerations

- **Latency**: There will be some latency from SSH + pipe overhead (usually <1 second)
- **Bandwidth**: Text logs are small, but high-volume logs may slow down
- **SSH compression**: Add `-C` flag to SSH for compression if bandwidth is limited

```bash
ssh -C user@server "tail -f /var/log/*.log" | logwatch --stdin
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
ssh prod "tail -f /var/log/app/*.log" | logwatch --stdin -i "ERROR|EXCEPTION"

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
ssh staging "tail -f /var/log/app/*.log" | logwatch --stdin -i "ERROR"

# Terminal 2: Production
ssh prod "tail -f /var/log/app/*.log" | logwatch --stdin -i "ERROR"
```
