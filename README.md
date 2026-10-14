# Wield

A native Rust utility app for managing Windows 10 and Windows 11 (Home, Pro, Enterprise, Education, LTSC/IoT).
Single portable `.exe`, no runtime or installer needed.

## Tools

| Tool | Status |
|---|---|
| **Startup Manager** | ✅ Available |
| Privacy & Telemetry, App Debloater, Disk Cleanup, Update Control, Network Toolkit, Uninstaller, System Info | Planned |

### Startup Manager

Finds everything that starts with Windows:

- **Registry Run keys**: HKCU/HKLM `Run` (64- and 32-bit), plus `RunOnce` and policy `Run` (read-only)
- **Startup folders**: per-user and all-users, with shortcut targets resolved
- **Services**: every *Automatic* service, with delayed-start detection and the real `ServiceDll` for svchost services
- **Scheduled tasks**: every task with an *at startup* or *at log on* trigger
- **Winlogon**: `Shell`, `Userinit` and `AppInit_DLLs` (read-only; flagged if they differ from the Windows defaults)

Each entry is rated on two scales:

- **Criticality**: what breaks if you disable it.
  - 🟢 *Safe to disable*: Windows doesn't need it. Switches off with one click.
  - 🟠 *Use caution*: turns off a Windows feature, hardware helper or updates. You must tick a confirmation box.
  - 🔴 *Critical*: Windows depends on it (RPC, DHCP, DNS, audio, sign-in, Defender…). You must type `DISABLE` to confirm.

  Ratings come from a curated knowledge base (~150 services, Windows task folders, common apps), then vendor heuristics
  (security, GPU/audio/touchpad drivers, updaters). Entries whose program no longer exists are flagged as leftovers.
- **Impact** (High / Medium / Low): estimated from live memory use, program size, known heavy apps and start mode.
  Windows doesn't expose its own measured startup impact through any API.

**Safety**

- Disabling uses the same reversible mechanisms as Windows itself:
  - Run keys and Startup folders: Task Manager's `StartupApproved` flags. Nothing is deleted.
  - Services: start type is changed to *Manual* (recommended) or *Disabled*.
  - Scheduled tasks: the task's *Enabled* flag.
- Every change is recorded in `%LOCALAPPDATA%\Wield\startup-journal.json`. **History** can revert any change exactly.
- **Restore point** button creates a System Restore point before risky changes.
- Without administrator rights the app is read-only for machine-wide entries. Use *Restart as administrator* (UAC prompt).

## Build

Requires the Rust MSVC toolchain.

```bash
cargo build --release
```

The binary is `target/release/wield.exe`. Run `wield.exe --tool startup` to open the Startup Manager directly.

```bash
cargo test
cargo test live_scan -- --ignored --nocapture
```

The second command scans the real machine and prints every entry with its rating.

## Releases

CI builds and tests every push and pull request. To publish a release, push a version tag:

```bash
git tag v0.1.0
```

```bash
git push origin v0.1.0
```

The Release workflow attaches `wield-<tag>-windows-x64.exe` and its SHA-256 checksum to a GitHub Release.
