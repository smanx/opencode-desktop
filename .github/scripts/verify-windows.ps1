#!/usr/bin/env pwsh
# Verify the installed OpenCode Desktop app on Windows:
#   launch the app, confirm it keeps running, confirm the opencode server +
#   web UI is reachable, capture a screenshot, and emit a PASS/FAIL verdict.

param(
    [int]$Port = 4096,
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

# --- locate the installed app -------------------------------------------------
$candidates = @(
    (Join-Path $env:LOCALAPPDATA 'opencode-desktop\opencode-desktop.exe'),
    (Join-Path ${env:ProgramFiles} 'opencode-desktop\opencode-desktop.exe')
)
$installed = $candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $installed) {
    $found = Get-ChildItem -Path $env:LOCALAPPDATA, ${env:ProgramFiles} `
        -Filter 'opencode-desktop.exe' -Recurse -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($found) { $installed = $found.FullName }
}
if (-not $installed) {
    Write-Host '::error::Installed opencode-desktop.exe was not found after package install.'
    exit 1
}
Write-Host "Launching installed app: $installed"

# --- launch -------------------------------------------------------------------
$appProc = Start-Process -FilePath $installed -PassThru
Write-Host "App launched, PID $($appProc.Id)"

function Save-Screenshot {
    param([string]$Path)
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -AssemblyName System.Drawing
    try {
        $vs = [System.Windows.Forms.SystemInformation]::VirtualScreen
        $bmp = New-Object System.Drawing.Bitmap($vs.Width, $vs.Height)
        $gfx = [System.Drawing.Graphics]::FromImage($bmp)
        $gfx.CopyFromScreen($vs.Left, $vs.Top, 0, 0, $bmp.Size)
        $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
        $gfx.Dispose()
        $bmp.Dispose()
        return $true
    } catch {
        Write-Warning "Full-screen capture failed: $($_.Exception.Message)"
    }
    # Fallback: PrintWindow into a bitmap. Works even when the desktop cannot
    # be captured directly (e.g. non-interactive runner sessions).
    try {
        if (-not ('Win32Capture' -as [type])) {
            Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class Win32Capture
{
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }

    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);

    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);
}
'@ -ErrorAction Stop
        }

        $hWnd = [IntPtr]::Zero
        $proc = Get-Process -Id $appProc.Id -ErrorAction SilentlyContinue
        if ($proc -and $proc.MainWindowHandle -ne [IntPtr]::Zero) {
            $hWnd = $proc.MainWindowHandle
        }
        if ($hWnd -eq [IntPtr]::Zero) {
            # the real window may live in the WebView2 host process
            $webviewProc = Get-Process -Name 'msedgewebview2' -ErrorAction SilentlyContinue |
                Where-Object { $_.MainWindowHandle -ne [IntPtr]::Zero } |
                Select-Object -First 1
            if ($webviewProc) { $hWnd = $webviewProc.MainWindowHandle }
        }
        if ($hWnd -ne [IntPtr]::Zero) {
            [Win32Capture]::SetForegroundWindow($hWnd) | Out-Null
            Start-Sleep -Seconds 1
            $rect = New-Object Win32Capture+RECT
            if ([Win32Capture]::GetWindowRect($hWnd, [ref]$rect)) {
                $w = $rect.Right - $rect.Left
                $h = $rect.Bottom - $rect.Top
                if ($w -gt 0 -and $h -gt 0) {
                    $bmp = New-Object System.Drawing.Bitmap($w, $h)
                    $gfx = [System.Drawing.Graphics]::FromImage($bmp)
                    $hdc = $gfx.GetHdc()
                    $ok = [Win32Capture]::PrintWindow($hWnd, $hdc, 2) # PW_RENDERFULLCONTENT
                    $gfx.ReleaseHdc($hdc)
                    if ($ok) {
                        $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
                        $gfx.Dispose()
                        $bmp.Dispose()
                        return $true
                    }
                    $gfx.Dispose()
                    $bmp.Dispose()
                }
            }
        }
    } catch {
        Write-Warning "PrintWindow fallback failed: $($_.Exception.Message)"
    }
    return $false
}

# (e) screenshot timeline: at launch (t=0), then at 5s and 10s
$shotOk = $false
$shotPaths = @(
    (Join-Path $PWD 'screenshot-windows-1.png'),
    (Join-Path $PWD 'screenshot-windows-2.png'),
    (Join-Path $PWD 'screenshot-windows-3.png')
)
$shotTimes = @(0, 5, 10)
for ($i = 0; $i -lt $shotPaths.Count; $i++) {
    if ($i -gt 0) { Start-Sleep -Seconds ($shotTimes[$i] - $shotTimes[$i - 1]) }
    if (Save-Screenshot -Path $shotPaths[$i]) {
        $shotOk = $true
        Write-Host "Screenshot ${i} (t=$($shotTimes[$i])s): $($shotPaths[$i])"
    }
}

# --- poll for the opencode server --------------------------------------------
$serverOk = $false
$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
while ((Get-Date) -lt $deadline) {
    if ($appProc.HasExited) {
        Write-Host "App process exited early (exit code $($appProc.ExitCode))."
        break
    }
    try {
        $resp = Invoke-WebRequest -Uri "http://127.0.0.1:$Port/" -Headers $authHeaders -TimeoutSec 5 -UseBasicParsing
        $low = $resp.Content.ToLowerInvariant()
        if ($resp.StatusCode -lt 400 -and ($low -match 'opencode' -or $low -match 'healthy')) {
            $serverOk = $true
            Write-Host "opencode server is responding on port $Port."
            break
        }
    } catch {
        # server not up yet; keep polling
    }
    Start-Sleep -Seconds 3
}

$procAlive = -not $appProc.HasExited

# --- did the webview load the UI? check for an established TCP connection -----
$uiConnected = $false
try {
    if (Get-NetTCPConnection -RemotePort $Port -State Established -ErrorAction Stop) {
        $uiConnected = $true
    }
} catch {
    $uiConnected = $false
}

# --- verdict ------------------------------------------------------------------
if ($procAlive -and $serverOk) {
    $verdict = 'PASS'
    $detail = "App stayed running and the opencode server responded on port $Port."
} elseif ($serverOk) {
    $verdict = 'FAIL'
    $detail = "opencode server responded on port $Port, but the app process exited."
} elseif ($procAlive) {
    $verdict = 'FAIL'
    $detail = "App process is alive but the opencode server did not respond on port $Port."
} else {
    $verdict = 'FAIL'
    $detail = "App process exited and the opencode server did not respond on port $Port."
}

# --- collect the app's opencode-web.log (the spawned opencode writes here) ----
$ocLog = @(
    (Join-Path $env:APPDATA 'com.opencode.desktop\logs\opencode-web.log'),
    (Join-Path $env:LOCALAPPDATA 'com.opencode.desktop\logs\opencode-web.log')
) | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if ($ocLog) { Write-Host "opencode-web.log: $ocLog" } else { Write-Host 'opencode-web.log: not found' }

$reportLines = @(
    '## OpenCode Desktop verification report (Windows)'
    ''
    "- Installed app: $installed"
    "- App process running: $procAlive"
    "- opencode server reachable (port $Port): $serverOk"
    "- UI loaded (TCP connection to port $Port): $uiConnected"
    "- Screenshots: screenshot-windows-1.png (t=0s), -2.png (t=5s), -3.png (t=10s)"
    "- Screenshot captured: $shotOk"
    "- Verdict: **$verdict**"
    "- Detail: $detail"
)
if ($ocLog) {
    $reportLines += @('', '### opencode-web.log (tail)', '```', (Get-Content -LiteralPath $ocLog -Tail 40), '```')
}
$report = $reportLines -join "`n"
$report | Set-Content -Path (Join-Path $PWD 'report.txt') -Encoding utf8
Write-Host ''
Write-Host '----- VERIFICATION REPORT -----'
Write-Host $report
Write-Host '--------------------------------'

if ($env:GITHUB_STEP_SUMMARY) {
    $report | Add-Content -Path $env:GITHUB_STEP_SUMMARY -Encoding utf8
}
if ($env:GITHUB_OUTPUT) {
    "verdict=$verdict" | Add-Content -Path $env:GITHUB_OUTPUT -Encoding utf8
}

if ($verdict -ne 'PASS') {
    Write-Host '::error::OpenCode Desktop verification FAILED on Windows.'
    exit 1
}
Write-Host '::notice::OpenCode Desktop verification PASSED on Windows.'
