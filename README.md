# Wield

A native Rust utility app for managing Windows 10 and Windows 11 (Home, Pro, Enterprise, Education, LTSC/IoT).
Single portable `.exe`, no runtime or installer needed.
Download it from [Releases](https://github.com/plaintextlab/wield/releases).

![Startup Manager](docs/screenshots/startup.png)

## Tools

![Home screen](docs/screenshots/home.png)

| Tool | Status |
|---|---|
| **Startup Manager** | ✅ Available |
| **Privacy & Telemetry** | ✅ Available |
| App Debloater, Disk Cleanup, Update Control, Network Toolkit, Uninstaller, System Info | Planned |

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

### Privacy & Telemetry

![Privacy & Telemetry](docs/screenshots/privacy.png)

42 privacy settings in six groups, each shown with what it does and what you give up:

- **Telemetry & diagnostics**: diagnostic data level, the DiagTrack and dmwappushservice services, telemetry
  scheduled tasks, CEIP, app telemetry and inventory, Error Reporting, feedback prompts, tailored experiences,
  inking/typing data.
- **Ads & suggestions**: advertising ID, auto-installed promoted apps, tips, Settings and lock screen promotions,
  post-update welcome pages, File Explorer sync ads, consumer experiences.
- **Search**: Bing web results in Start, search highlights, cloud content search, Cortana.
- **Activity, speech & typing**: activity history, cross-device clipboard, online speech recognition,
  inking & typing personalization, app launch tracking.
- **AI features**: Recall snapshots (24H2+ Copilot+ PCs) and Windows Copilot.
- **App permissions**: block Store apps from account info, app diagnostics, location, camera, microphone,
  notifications, contacts, calendar, call history, email, messages and radios.

How it behaves:

- A **privacy score** counts how many of the 29 recommended protections are on. **Apply all recommended** turns
  them all on after showing you the list.
- It knows your edition. For example, it sets diagnostic data to *Security* on Enterprise/Education/LTSC and to
  *Required* on Home/Pro, and hides settings that don't exist on your build.
- Switching a setting off restores the Windows default. The first time Wield touches a value it saves the
  original to `%LOCALAPPDATA%\Wield\privacy-backup.json`, and **Restore original settings** puts everything back
  exactly as it was.
- Personal (HKCU) settings work without admin rights. All-users settings are marked with a lock.

## Build

Requires the Rust MSVC toolchain.

```bash
cargo build --release
```

The binary is `target/release/wield.exe`. Run `wield.exe --tool startup` or `wield.exe --tool privacy` to open a tool directly.

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
