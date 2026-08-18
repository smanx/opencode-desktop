#!/usr/bin/env pwsh
# Boot the globally installed `opencode` tool once and confirm its server + web
# UI comes up. This both verifies the tool works in the CI environment and
# pre-warms the opencode profile (first boot is slow: it initializes config,
# fetches models, etc.), so the app-under-test boots quickly afterwards.

param(
    [int]$Port = 4196,
    [int]$TimeoutSeconds = 240
)

$ErrorActionPreference = 'Continue'

# Send Basic auth when the environment is configured with a server password
# (matches what the app itself does). In clean CI environments no password is
# set and the server is unsecured.
$authHeaders = @{}
if ($env:OPENCODE_SERVER_PASSWORD) {
    $authUser = if ($env:OPENCODE_SERVER_USERNAME) { $env:OPENCODE_SERVER_USERNAME } else { 'opencode' }
    $token = [Convert]::ToBase64String(
        [System.Text.Encoding]::UTF8.GetBytes("$authUser`:$($env:OPENCODE_SERVER_PASSWORD)")
    )
    $authHeaders['Authorization'] = "Basic $token"
}

$logOut = Join-Path $PWD 'smoke-opencode.log'
$logErr = Join-Path $PWD 'smoke-opencode.err.log'

Write-Host "Smoke-testing opencode server on port $Port (timeout ${TimeoutSeconds}s)..."

$proc = $null
try {
    if ($IsWindows) {
        $proc = Start-Process -FilePath 'cmd' `
            -ArgumentList '/c', 'opencode', 'serve', '--port', "$Port" `
            -RedirectStandardOutput $logOut -RedirectStandardError $logErr `
            -WindowStyle Hidden -PassThru
    } else {
        $opencode = (Get-Command 'opencode' -ErrorAction Stop).Source
        $proc = Start-Process -FilePath $opencode `
            -ArgumentList 'serve', '--port', "$Port" `
            -RedirectStandardOutput $logOut -RedirectStandardError $logErr `
            -PassThru
    }
} catch {
    Write-Host "::error::Failed to start opencode: $($_.Exception.Message)"
    exit 1
}
Write-Host "opencode started, PID $($proc.Id)"

# --- poll for the opencode server ---------------------------------------------
$serverOk = $false
$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
while ((Get-Date) -lt $deadline) {
    try {
        $resp = Invoke-WebRequest -Uri "http://127.0.0.1:$Port/" -Headers $authHeaders -TimeoutSec 5 -UseBasicParsing
        $low = $resp.Content.ToLowerInvariant()
        if ($resp.StatusCode -lt 400 -and ($low -match 'opencode' -or $low -match 'healthy')) {
            $serverOk = $true
            Write-Host "opencode server came up on port $Port."
            break
        }
    } catch {
        # not ready yet; keep polling
    }
    Start-Sleep -Seconds 3
}

if (-not $serverOk) {
    Write-Host '::error::opencode did not respond in time. Tail of logs:'
    foreach ($f in @($logOut, $logErr)) {
        if (Test-Path -LiteralPath $f) {
            Write-Host "--- $f ---"
            Get-Content -LiteralPath $f -Tail 40
        }
    }
    $logsPresent = (Test-Path -LiteralPath $logOut) -or (Test-Path -LiteralPath $logErr)
    if (-not $logsPresent) { Write-Host '(no logs were produced)' }
    Write-Host '::error::opencode tool smoke test FAILED (opencode serve did not come up).'
    exit 1
}

# --- shut opencode down so the app-under-test starts clean ---------------------
if ($IsWindows) {
    try {
        $listeners = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction Stop
        $pids = $listeners.OwningProcess | Sort-Object -Unique
        foreach ($pid in $pids) {
            cmd /c "taskkill /PID $pid /T /F" 2>&1 | Out-Null
        }
    } catch { }
    if ($proc -and -not $proc.HasExited) {
        try { cmd /c "taskkill /PID $($proc.Id) /T /F" 2>&1 | Out-Null } catch { }
    }
} else {
    if ($proc -and -not $proc.HasExited) {
        try { Stop-Process -Id $proc.Id -Force -ErrorAction Stop } catch { }
    }
    # kill any remaining opencode worker processes
    pkill -f 'opencode-ai' 2>$null | Out-Null
    pkill -f 'opencode serve --port' 2>$null | Out-Null
}

# wait for the smoke server to fully release the port
$deadline = (Get-Date).AddSeconds(20)
while ((Get-Date) -lt $deadline) {
    $still = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue
    if (-not $still) { break }
    Start-Sleep -Seconds 1
}

Write-Host '::notice::opencode tool smoke test PASSED (opencode serve came up; profile pre-warmed).'
