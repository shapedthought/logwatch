# Script to generate test logs for LogWatch (PowerShell port of generate_test_logs.sh)
# Works in Windows PowerShell 5.1 and PowerShell 7+.
# Usage:  .\generate_test_logs.ps1

$ErrorActionPreference = 'Stop'

# Create test log directories (absolute path, so background jobs resolve it too)
$TestDir = Join-Path (Get-Location) 'test_logs'
foreach ($sub in 'app', 'api', 'database') {
    New-Item -ItemType Directory -Force -Path (Join-Path $TestDir $sub) | Out-Null
}

Write-Host "Creating test log directories in $TestDir" -ForegroundColor Green

# One writer per file. Each appends a random log line at a random 1-5s interval,
# mirroring the three parallel writers in the shell script.
$writer = {
    param($FilePath, $Prefix)

    $levels = 'DEBUG', 'INFO', 'WARN', 'ERROR'
    $messages = @(
        'Database connection successful'
        'Request processed in 150ms'
        'Cache miss for key user_123'
        'Authentication successful'
        'Connection timeout after 30s'
        'Invalid input detected'
        'Retrying failed operation'
        'Health check passed'
        'Configuration reloaded'
        'Memory usage at 75%'
    )

    while ($true) {
        $level = $levels | Get-Random
        $msg = $messages | Get-Random
        $timestamp = Get-Date -Format 'yyyy-MM-dd HH:mm:ss'
        Add-Content -Path $FilePath -Value "$timestamp [$level] ${Prefix}: $msg"
        Start-Sleep -Seconds (Get-Random -Minimum 1 -Maximum 6)
    }
}

Write-Host "Starting log generation..." -ForegroundColor Yellow
Write-Host "Press Ctrl+C to stop"
Write-Host ""
Write-Host "You can now run logwatch in another terminal:" -ForegroundColor Cyan
Write-Host "  cargo run -- -d .\test_logs"
Write-Host "  cargo run -- -d .\test_logs -i ERROR -i WARN"
Write-Host ""

# Launch the three writers as background jobs and clean them up on exit / Ctrl+C.
$jobs = @(
    Start-Job -ScriptBlock $writer -ArgumentList (Join-Path $TestDir 'app\server.log'), 'Server'
    Start-Job -ScriptBlock $writer -ArgumentList (Join-Path $TestDir 'api\requests.log'), 'API'
    Start-Job -ScriptBlock $writer -ArgumentList (Join-Path $TestDir 'database\queries.log'), 'DB'
)

try {
    # Block until interrupted; the jobs loop forever, so this waits for Ctrl+C.
    Wait-Job -Job $jobs | Out-Null
}
finally {
    Write-Host "`nStopping log generation..." -ForegroundColor Yellow
    $jobs | Stop-Job -ErrorAction SilentlyContinue
    $jobs | Remove-Job -Force -ErrorAction SilentlyContinue
}
