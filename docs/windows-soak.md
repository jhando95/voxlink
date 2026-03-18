# Windows Release Soak

This harness is for repeatable release-build stability runs on Windows using the real `app_desktop.exe` and `signaling_server.exe` binaries.

What it covers:

- `idle-ui`: launches the GUI client with auto-connect and update checks disabled, samples memory and CPU, then closes it.
- `space_channel_soak`: starts one owner and two participants in automation mode, joins the same voice channel, and verifies audio relay.
- `space_text_chat_soak`: starts one owner and one participant in automation mode, selects a text channel, and verifies a text burst.
- `space_screen_share_soak`: starts one owner and one participant in automation mode, joins the same voice channel, and verifies synthetic screen-frame relay.

The script lives at [release-soak.ps1](/Users/jph/Voiceapp/workspace_template/tools/windows/release-soak.ps1).

## Run

From a Windows PowerShell prompt in the workspace root:

```powershell
powershell -ExecutionPolicy Bypass -File .\tools\windows\release-soak.ps1
```

Useful options:

```powershell
powershell -ExecutionPolicy Bypass -File .\tools\windows\release-soak.ps1 -SkipBuild
powershell -ExecutionPolicy Bypass -File .\tools\windows\release-soak.ps1 -IdleSeconds 30 -HoldMs 4000 -SampleMs 250
```

Artifacts are written under `artifacts\soak\<timestamp>\`.

Important outputs:

- `server\resource-summary.json`
- `idle-ui\resource-summary.json`
- `<scenario>\scenario-summary.json`
- `<scenario>\samples\*.summary.json`
- `<scenario>\reports\*.json`

## Crash Reports

The desktop app now writes crash bundles into the Voxlink data directory under `crashes\`.

Each bundle includes:

- a JSON crash report with panic message, location, thread, PID, and backtrace
- a snapshot copy of the current `voxlink.log`
- a `latest-crash.txt` pointer to the newest crash report

The app logs the crash directory on startup, and the panic hook prints the written crash-report path before the process exits.

## Notes

- The automation scenarios bypass normal config/UI startup, so they do not race on persisted settings when multiple clients run at once.
- `idle-ui` is intentionally non-automated so you can measure the actual GUI process footprint.
- `space_screen_share_soak` uses synthetic screen-frame payloads. It validates relay stability and resource behavior without depending on desktop capture permissions.
