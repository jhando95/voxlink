param(
    [string]$ServerAddr = "127.0.0.1:19090",
    [int]$IdleSeconds = 20,
    [int]$HoldMs = 2600,
    [int]$SampleMs = 500,
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$WorkspaceRoot = Resolve-Path (Join-Path $ScriptDir "..\..")
$Timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$OutputRoot = Join-Path $WorkspaceRoot "artifacts\soak\$Timestamp"
$TargetDir = Join-Path $WorkspaceRoot "target\release"
$ServerBin = Join-Path $TargetDir "signaling_server.exe"
$AppBin = Join-Path $TargetDir "app_desktop.exe"
$ServerUrl = "ws://$ServerAddr"

New-Item -ItemType Directory -Force -Path $OutputRoot | Out-Null

function Start-VoxlinkProcess {
    param(
        [string]$FilePath,
        [string]$WorkingDirectory,
        [hashtable]$Environment = @{},
        [string]$StdOutPath,
        [string]$StdErrPath
    )

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $FilePath
    $psi.WorkingDirectory = $WorkingDirectory
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true

    foreach ($entry in $Environment.GetEnumerator()) {
        $psi.EnvironmentVariables[$entry.Key] = [string]$entry.Value
    }

    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $psi
    $null = $process.Start()

    [pscustomobject]@{
        Process = $process
        StdOutTask = $process.StandardOutput.ReadToEndAsync()
        StdErrTask = $process.StandardError.ReadToEndAsync()
        StdOutPath = $StdOutPath
        StdErrPath = $StdErrPath
    }
}

function Stop-VoxlinkProcess {
    param($ManagedProcess)

    if ($null -eq $ManagedProcess) {
        return
    }

    $process = $ManagedProcess.Process
    if ($null -ne $process -and -not $process.HasExited) {
        $null = $process.CloseMainWindow()
        if (-not $process.WaitForExit(5000)) {
            $process.Kill()
            $process.WaitForExit()
        }
    }

    if ($ManagedProcess.StdOutTask) {
        $ManagedProcess.StdOutTask.GetAwaiter().GetResult() | Set-Content -Path $ManagedProcess.StdOutPath
    }
    if ($ManagedProcess.StdErrTask) {
        $ManagedProcess.StdErrTask.GetAwaiter().GetResult() | Set-Content -Path $ManagedProcess.StdErrPath
    }
}

function Start-ResourceSampler {
    param(
        [int]$Pid,
        [string]$SamplePath,
        [int]$IntervalMs
    )

    Start-Job -ScriptBlock {
        param($Pid, $SamplePath, $IntervalMs)

        $samples = New-Object System.Collections.Generic.List[object]
        while ($true) {
            try {
                $proc = Get-Process -Id $Pid -ErrorAction Stop
            } catch {
                break
            }

            $samples.Add([pscustomobject]@{
                timestamp = (Get-Date).ToString("o")
                pid = $proc.Id
                working_set_mb = [math]::Round($proc.WorkingSet64 / 1MB, 2)
                private_mb = [math]::Round($proc.PrivateMemorySize64 / 1MB, 2)
                cpu_seconds = [math]::Round($proc.CPU, 2)
                handles = $proc.Handles
                threads = $proc.Threads.Count
            })

            Start-Sleep -Milliseconds $IntervalMs
        }

        $samples | ConvertTo-Json -Depth 4 | Set-Content -Path $SamplePath
    } -ArgumentList $Pid, $SamplePath, $IntervalMs
}

function Stop-ResourceSampler {
    param($Job)

    if ($null -eq $Job) {
        return
    }

    Wait-Job $Job | Out-Null
    Receive-Job $Job | Out-Null
    Remove-Job $Job | Out-Null
}

function Write-SampleSummary {
    param(
        [string]$SamplePath,
        [string]$SummaryPath
    )

    if (-not (Test-Path $SamplePath)) {
        return
    }

    $raw = Get-Content -Path $SamplePath -Raw
    if ([string]::IsNullOrWhiteSpace($raw)) {
        return
    }

    $samples = $raw | ConvertFrom-Json
    if ($samples -isnot [System.Array]) {
        $samples = @($samples)
    }
    if ($samples.Count -eq 0) {
        return
    }

    $summary = [pscustomobject]@{
        sample_count = $samples.Count
        peak_working_set_mb = [math]::Round(($samples | Measure-Object -Property working_set_mb -Maximum).Maximum, 2)
        peak_private_mb = [math]::Round(($samples | Measure-Object -Property private_mb -Maximum).Maximum, 2)
        peak_handles = [int](($samples | Measure-Object -Property handles -Maximum).Maximum)
        peak_threads = [int](($samples | Measure-Object -Property threads -Maximum).Maximum)
        cpu_seconds_delta = [math]::Round(($samples[-1].cpu_seconds - $samples[0].cpu_seconds), 2)
    }

    $summary | ConvertTo-Json -Depth 4 | Set-Content -Path $SummaryPath
}

function Wait-ForReports {
    param(
        [string[]]$ReportPaths,
        [int]$TimeoutSeconds = 60
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        $missing = $ReportPaths | Where-Object { -not (Test-Path $_) }
        if ($missing.Count -eq 0) {
            return
        }
        Start-Sleep -Milliseconds 250
    }

    throw "Timed out waiting for reports: $($ReportPaths -join ', ')"
}

function Read-JsonFile {
    param([string]$Path)
    Get-Content -Path $Path -Raw | ConvertFrom-Json
}

function Build-Workspace {
    if ($SkipBuild) {
        return
    }

    Push-Location $WorkspaceRoot
    try {
        cargo build --release -p signaling_server -p app_desktop
    } finally {
        Pop-Location
    }
}

function Start-Server {
    $serverDir = Join-Path $OutputRoot "server"
    New-Item -ItemType Directory -Force -Path $serverDir | Out-Null

    $server = Start-VoxlinkProcess `
        -FilePath $ServerBin `
        -WorkingDirectory $WorkspaceRoot `
        -Environment @{
            PV_ADDR = $ServerAddr
            PV_DB_PATH = (Join-Path $serverDir "voxlink.db")
        } `
        -StdOutPath (Join-Path $serverDir "stdout.log") `
        -StdErrPath (Join-Path $serverDir "stderr.log")

    $sampler = Start-ResourceSampler -Pid $server.Process.Id -SamplePath (Join-Path $serverDir "resource-samples.json") -IntervalMs $SampleMs
    Start-Sleep -Seconds 2
    return [pscustomobject]@{ Managed = $server; Sampler = $sampler; Dir = $serverDir }
}

function Stop-Server {
    param($ServerState)

    Stop-VoxlinkProcess $ServerState.Managed
    Stop-ResourceSampler $ServerState.Sampler
    Write-SampleSummary -SamplePath (Join-Path $ServerState.Dir "resource-samples.json") -SummaryPath (Join-Path $ServerState.Dir "resource-summary.json")
}

function Run-IdleUiScenario {
    $scenarioDir = Join-Path $OutputRoot "idle-ui"
    New-Item -ItemType Directory -Force -Path $scenarioDir | Out-Null

    $app = Start-VoxlinkProcess `
        -FilePath $AppBin `
        -WorkingDirectory $WorkspaceRoot `
        -Environment @{
            VOXLINK_DISABLE_AUTO_CONNECT = "1"
            VOXLINK_DISABLE_UPDATE_CHECK = "1"
        } `
        -StdOutPath (Join-Path $scenarioDir "stdout.log") `
        -StdErrPath (Join-Path $scenarioDir "stderr.log")

    $sampler = Start-ResourceSampler -Pid $app.Process.Id -SamplePath (Join-Path $scenarioDir "resource-samples.json") -IntervalMs $SampleMs
    Start-Sleep -Seconds $IdleSeconds
    Stop-VoxlinkProcess $app
    Stop-ResourceSampler $sampler
    Write-SampleSummary -SamplePath (Join-Path $scenarioDir "resource-samples.json") -SummaryPath (Join-Path $scenarioDir "resource-summary.json")
}

function Run-AutomationScenario {
    param(
        [string]$ScenarioName,
        [int]$ParticipantCount,
        [bool]$SendAudio = $false,
        [bool]$ExpectAudio = $false,
        [int]$MessageCount = 0,
        [int]$ScreenFrameCount = 0
    )

    $scenarioDir = Join-Path $OutputRoot $ScenarioName
    $reportDir = Join-Path $scenarioDir "reports"
    $sampleDir = Join-Path $scenarioDir "samples"
    New-Item -ItemType Directory -Force -Path $reportDir, $sampleDir | Out-Null

    $sharedPath = Join-Path $scenarioDir "shared.json"
    $managed = @()
    $samplers = @()
    $reportPaths = @()

    $ownerReport = Join-Path $reportDir "owner.json"
    $reportPaths += $ownerReport
    $owner = Start-VoxlinkProcess `
        -FilePath $AppBin `
        -WorkingDirectory $WorkspaceRoot `
        -Environment @{
            VOXLINK_AUTOMATION_SCENARIO = $ScenarioName
            VOXLINK_AUTOMATION_ROLE = "owner"
            VOXLINK_AUTOMATION_SERVER_URL = $ServerUrl
            VOXLINK_AUTOMATION_USER_NAME = "Owner"
            VOXLINK_AUTOMATION_SPACE_NAME = "Release Soak"
            VOXLINK_AUTOMATION_SHARED_PATH = $sharedPath
            VOXLINK_AUTOMATION_REPORT_PATH = $ownerReport
            VOXLINK_AUTOMATION_HOLD_MS = $HoldMs
            VOXLINK_AUTOMATION_INVITE_TIMEOUT_MS = 12000
            VOXLINK_AUTOMATION_EXPECT_PEERS = $ParticipantCount
            VOXLINK_AUTOMATION_EXPECT_AUDIO = $(if ($ExpectAudio) { "1" } else { "0" })
            VOXLINK_AUTOMATION_SEND_AUDIO = $(if ($SendAudio) { "1" } else { "0" })
            VOXLINK_AUTOMATION_MESSAGE_COUNT = $MessageCount
            VOXLINK_AUTOMATION_SCREEN_FRAME_COUNT = $ScreenFrameCount
        } `
        -StdOutPath (Join-Path $scenarioDir "owner.stdout.log") `
        -StdErrPath (Join-Path $scenarioDir "owner.stderr.log")
    $managed += $owner
    $samplers += Start-ResourceSampler -Pid $owner.Process.Id -SamplePath (Join-Path $sampleDir "owner.json") -IntervalMs $SampleMs

    Start-Sleep -Milliseconds 500

    for ($index = 1; $index -le $ParticipantCount; $index++) {
        $reportPath = Join-Path $reportDir "participant-$index.json"
        $reportPaths += $reportPath
        $participant = Start-VoxlinkProcess `
            -FilePath $AppBin `
            -WorkingDirectory $WorkspaceRoot `
            -Environment @{
                VOXLINK_AUTOMATION_SCENARIO = $ScenarioName
                VOXLINK_AUTOMATION_ROLE = "participant"
                VOXLINK_AUTOMATION_SERVER_URL = $ServerUrl
                VOXLINK_AUTOMATION_USER_NAME = "Participant$index"
                VOXLINK_AUTOMATION_SPACE_NAME = "Release Soak"
                VOXLINK_AUTOMATION_SHARED_PATH = $sharedPath
                VOXLINK_AUTOMATION_REPORT_PATH = $reportPath
                VOXLINK_AUTOMATION_HOLD_MS = $HoldMs
                VOXLINK_AUTOMATION_INVITE_TIMEOUT_MS = 12000
                VOXLINK_AUTOMATION_EXPECT_PEERS = $ParticipantCount
                VOXLINK_AUTOMATION_EXPECT_AUDIO = $(if ($ExpectAudio) { "1" } else { "0" })
                VOXLINK_AUTOMATION_SEND_AUDIO = "0"
                VOXLINK_AUTOMATION_MESSAGE_COUNT = $MessageCount
                VOXLINK_AUTOMATION_SCREEN_FRAME_COUNT = $ScreenFrameCount
            } `
            -StdOutPath (Join-Path $scenarioDir "participant-$index.stdout.log") `
            -StdErrPath (Join-Path $scenarioDir "participant-$index.stderr.log")
        $managed += $participant
        $samplers += Start-ResourceSampler -Pid $participant.Process.Id -SamplePath (Join-Path $sampleDir "participant-$index.json") -IntervalMs $SampleMs
        Start-Sleep -Milliseconds 300
    }

    foreach ($proc in $managed) {
        $proc.Process.WaitForExit()
    }

    foreach ($proc in $managed) {
        Stop-VoxlinkProcess $proc
    }
    foreach ($job in $samplers) {
        Stop-ResourceSampler $job
    }

    foreach ($sample in Get-ChildItem -Path $sampleDir -Filter "*.json") {
        $summaryPath = [System.IO.Path]::ChangeExtension($sample.FullName, ".summary.json")
        Write-SampleSummary -SamplePath $sample.FullName -SummaryPath $summaryPath
    }

    Wait-ForReports -ReportPaths $reportPaths

    $reports = $reportPaths | ForEach-Object { Read-JsonFile $_ }
    $failed = $reports | Where-Object { -not $_.ok }
    if ($failed.Count -gt 0) {
        $failed | ConvertTo-Json -Depth 6 | Set-Content -Path (Join-Path $scenarioDir "failures.json")
        throw "Scenario $ScenarioName reported failures"
    }

    $scenarioSummary = [pscustomobject]@{
        scenario = $ScenarioName
        reports = $reports
    }
    $scenarioSummary | ConvertTo-Json -Depth 6 | Set-Content -Path (Join-Path $scenarioDir "scenario-summary.json")
}

Build-Workspace

if (-not (Test-Path $ServerBin)) {
    throw "Missing server binary at $ServerBin"
}
if (-not (Test-Path $AppBin)) {
    throw "Missing desktop binary at $AppBin"
}

$server = Start-Server
try {
    Run-IdleUiScenario
    Run-AutomationScenario -ScenarioName "space_channel_soak" -ParticipantCount 2 -SendAudio $true -ExpectAudio $true
    Run-AutomationScenario -ScenarioName "space_text_chat_soak" -ParticipantCount 1 -MessageCount 30
    Run-AutomationScenario -ScenarioName "space_screen_share_soak" -ParticipantCount 1 -ScreenFrameCount 24
} finally {
    Stop-Server $server
}

$finalSummary = [pscustomobject]@{
    output_root = $OutputRoot
    server_addr = $ServerAddr
    idle_seconds = $IdleSeconds
    hold_ms = $HoldMs
    sample_ms = $SampleMs
}
$finalSummary | ConvertTo-Json -Depth 4 | Set-Content -Path (Join-Path $OutputRoot "run-summary.json")

Write-Host "Voxlink soak artifacts written to $OutputRoot"
