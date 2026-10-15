# Wield

A native Rust utility app for managing Windows 10 and Windows 11 (Home, Pro, Enterprise, Education, LTSC/IoT).
Single portable `.exe`, no runtime or installer needed.
Download it from [Releases](https://github.com/plaintextlab/wield/releases).

Uses the [Gruvbox](https://github.com/morhetz/gruvbox) color scheme, in dark and light. Switch with the sun/moon button
at the bottom of the sidebar; your choice is remembered.

![Startup Manager](docs/screenshots/startup.png)

## Tools

![Home screen](docs/screenshots/home.png)

| Tool | Status |
|---|---|
| **Startup Manager** | ✅ Available |
| **Privacy & Telemetry** | ✅ Available |
| **App Debloater** | ✅ Available |
| **Network Toolkit** | ✅ Available |
| Disk Cleanup, Update Control, Uninstaller, System Info | Planned |

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

### App Debloater

![App Debloater](docs/screenshots/debloat.png)

Lists every Store/MSIX app installed for your account, using the Windows `PackageManager` API rather than PowerShell.
Each app shows its publisher, size and source, and gets one of four verdicts:

- 🟢 **Recommended to remove**: preinstalled promotions and retired apps, such as Candy Crush, TikTok, Spotify, Disney+,
  Microsoft News, Tips, Solitaire, Clipchamp, Mixed Reality Portal, Movies & TV and Cortana.
- 🔵 **Optional**: fine to remove if you don't use it (Weather, Sticky Notes, Phone Link, Teams, Xbox Game Bar…).
- 🟠 **Use caution**: removing it breaks something, such as image/video codecs, Xbox sign-in for PC games,
  GPU/audio/touchpad control panels, Photos, Calculator or Snipping Tool. You must tick a confirmation box.
- 🔴 **Keep**: Microsoft Store, App Installer (winget), Windows Security, Edge and the app runtimes. You must type
  `REMOVE` to confirm.

Windows-protected system components are hidden by default and can't be selected.

- **Select recommended** checks all bloat in one click. The removal dialog lists every app and its verdict first.
- Remove for **your account only** (no admin needed), or as administrator for **all accounts**. As administrator you
  can also stop an app from reinstalling for new accounts and after feature updates.
- Every removal is logged to `%LOCALAPPDATA%\Wield\debloat-journal.json`. **History → Reinstall** re-registers the
  app from its files when Windows still has them, and otherwise opens its Microsoft Store page.

### Network Toolkit

![Network Toolkit](docs/screenshots/network.png)

![Network Toolkit in Gruvbox light](docs/screenshots/network-light.png)

- **Activity**: every process with network sockets, plus a live whole-PC throughput graph.
  - As administrator, each app gets live download/upload speeds and the bytes it has moved since Wield started
    watching. These come from Windows' per-connection TCP statistics.
  - Select an app to see each connection: the host name (from the DNS cache, otherwise reverse DNS), port/service,
    state and speed.
  - Known **telemetry**, **ads/tracking** and **crash-report** servers are flagged.
  - Wield can't read *what* is sent: almost all traffic is encrypted. It shows *who* each app talks to and *how much*.
- **Block or allow any app** with its switch. This creates Windows Firewall rules that block the program in both
  directions. Blocking Windows' own programs (svchost.exe and others) asks for confirmation first. Individual IPv4
  connections can be closed.
- **Ports**: which process listens on which TCP/UDP port, the service name, and whether the port is reachable from the
  network or only from this PC.
- **Firewall**: your blocked-programs list (add any `.exe` with Browse…, toggle, remove) and **open inbound ports**
  (TCP/UDP, single ports, ranges or lists). Every rule is in the "Wield" group, so it's easy to find in `wf.msc`.
- **Scanner**:
  - Discovers devices on your LAN with a parallel ping sweep plus the ARP cache. Shows IP, name, MAC, latency, the
    router, this PC, and randomized private MACs.
  - Includes a TCP **port scanner** (common ports, 1-1024, or a custom list). Only scan devices you own or are
    allowed to test.
- **Tools**:
  - Adapter details, and **DNS presets** per adapter (Automatic, Cloudflare, Cloudflare malware-blocking, Google,
    Quad9, AdGuard).
  - Flush the DNS cache, ping, and show your public IP (asks api.ipify.org only when you click).
  - A **hosts-file domain blocker**. The original hosts file is backed up the first time Wield edits it.
  - **Repairs**: renew IP, clear ARP cache, reset Winsock, reset TCP/IP.

Per-app traffic, blocking, opening ports, closing connections, DNS changes and most repairs need administrator rights.

## Build

Requires the Rust MSVC toolchain.

```bash
cargo build --release
```

The binary is `target/release/wield.exe`. Run `wield.exe --tool startup|privacy|debloat|network` to open a tool directly. For the Network Toolkit,
add `--tab ports|firewall|scanner|tools` to open a specific tab.

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
