//! Rates each startup entry for *criticality* (what breaks if you disable it) and
//! *impact* (how much it costs at boot/sign-in).
//!
//! Criticality comes from a curated knowledge base first, then from vendor / location
//! heuristics. Impact is an estimate built from live memory use, file size, known heavy
//! apps and start mode — Windows' own measured "startup impact" is not exposed by any API.

use super::model::{Criticality, Impact, SourceKind, StartupItem};
use crate::win::fileinfo::format_bytes;

use Criticality::{Critical as C, Important as I, Optional as O};

/// Windows services by service name (lowercase).
const SERVICES: &[(&str, Criticality, &str)] = &[
    // Core OS: Windows will not boot, sign in or run apps without these.
    ("rpcss", C, "Remote Procedure Call. Nearly every Windows component depends on it; Windows will not work without it."),
    ("dcomlaunch", C, "DCOM Server Process Launcher. Core COM/RPC infrastructure; disabling makes Windows unbootable."),
    ("rpceptmapper", C, "RPC Endpoint Mapper. Required by RPC and many system services."),
    ("lsm", C, "Local Session Manager. Manages user sessions; sign-in fails without it."),
    ("samss", C, "Security Accounts Manager. Sign-in and local security fail without it."),
    ("profsvc", C, "User Profile Service. You will not be able to sign in."),
    ("usermanager", C, "User Manager. Required for sign-in and the Start menu."),
    ("eventlog", C, "Windows Event Log. Many services fail to start without it."),
    ("plugplay", C, "Plug and Play. Device detection and drivers stop working."),
    ("power", C, "Power service. Power management, sleep and power plans depend on it."),
    ("brokerinfrastructure", C, "Background Tasks Infrastructure. Core broker for system and app background work."),
    ("systemeventsbroker", C, "System Events Broker. Required by Task Scheduler and app background tasks."),
    ("coremessagingregistrar", C, "CoreMessaging. Required by the Windows UI stack; the desktop may stop responding."),
    ("staterepository", C, "State Repository. Start menu, Settings and Store apps break without it."),
    ("schedule", C, "Task Scheduler. Windows maintenance and many apps rely on it (normally cannot be changed)."),
    ("winmgmt", C, "Windows Management Instrumentation. Many system tools, drivers and apps depend on WMI."),
    ("cryptsvc", C, "Cryptographic Services. Certificates, Windows Update, driver signing and HTTPS checks fail."),
    ("gpsvc", C, "Group Policy Client. Policies and some sign-in steps depend on it."),
    ("appinfo", C, "Application Information. Needed for UAC elevation; 'Run as administrator' stops working."),
    ("keyiso", C, "CNG Key Isolation. Protects private keys; Wi-Fi auth, certificates and sign-in can fail."),
    ("timebrokersvc", C, "Time Broker. Coordinates background work for apps and system components."),
    ("textinputmanagementsvc", C, "Text Input Management. Keyboard input, IME and the touch keyboard can stop working."),
    ("sppsvc", C, "Software Protection. Windows activation and licensing."),
    ("dispbrokerdesktopsvc", C, "Display Policy Service. Manages display configuration and connected monitors."),
    // Networking
    ("bfe", C, "Base Filtering Engine. Firewall, IPsec and VPNs depend on it; networking may break."),
    ("mpssvc", C, "Windows Defender Firewall. Disabling removes firewall protection and breaks some networking."),
    ("nsi", C, "Network Store Interface. Without it network connectivity fails."),
    ("dhcp", C, "DHCP Client. Your PC will not get an IP address on most networks."),
    ("dnscache", C, "DNS Client. Name resolution for every app depends on it."),
    ("wcmsvc", I, "Windows Connection Manager. Automatic network connect/disconnect decisions."),
    ("netprofm", I, "Network List Service. Apps may think you are offline; network settings break."),
    ("nlasvc", I, "Network Location Awareness. Apps may report 'No internet'; firewall profiles misbehave."),
    ("lanmanworkstation", I, "Workstation (SMB client). Needed to access network shares and by several services."),
    ("lanmanserver", I, "Server (SMB). Needed only if you share files or printers from this PC."),
    ("wlansvc", I, "WLAN AutoConfig. Wi-Fi stops working. Safe only on PCs that never use Wi-Fi."),
    ("dot3svc", I, "Wired AutoConfig. 802.1X authentication on wired networks."),
    ("iphlpsvc", O, "IP Helper. IPv6 transition tech (Teredo, 6to4); rarely needed on home PCs."),
    ("winhttpautoproxysvc", I, "WinHTTP Web Proxy Auto-Discovery. Proxy detection, used on corporate networks."),
    ("ikeext", I, "IKE and AuthIP. Needed for IPsec and many VPNs."),
    ("rasman", I, "Remote Access Connection Manager. Needed for VPN and dial-up connections."),
    ("sstpsvc", I, "SSTP Service. Needed for SSTP VPN connections."),
    ("policyagent", I, "IPsec Policy Agent. Needed for IPsec-based VPNs and domain policies."),
    ("netlogon", I, "Netlogon. Required on domain-joined PCs for domain sign-in."),
    ("sharedaccess", O, "Internet Connection Sharing. Only needed to share this PC's connection."),
    ("icssvc", O, "Mobile Hotspot. Only needed to use this PC as a hotspot."),
    ("ssdpsrv", O, "SSDP Discovery. UPnP device discovery on the local network."),
    ("upnphost", O, "UPnP Device Host. Only needed to host UPnP devices."),
    ("fdrespub", O, "Function Discovery Resource Publication. Makes this PC visible on the network."),
    ("lmhosts", O, "TCP/IP NetBIOS Helper. Legacy NetBIOS name resolution."),
    // Audio / devices / UI
    ("audiosrv", C, "Windows Audio. You will have no sound."),
    ("audioendpointbuilder", C, "Windows Audio Endpoint Builder. You will have no sound."),
    ("eventsystem", C, "COM+ Event System. Many services depend on it."),
    ("sens", I, "System Event Notification. Apps lose sign-in/network/power notifications."),
    ("themes", I, "Themes. Windows falls back to a basic look; some UI features break."),
    ("fontcache", I, "Font Cache. Apps start more slowly; fonts may render incorrectly."),
    ("shellhwdetection", I, "Shell Hardware Detection. AutoPlay and some device notifications stop."),
    ("hidserv", I, "Human Interface Device service. Keyboard media keys and hotkeys may stop working."),
    ("tabletinputservice", I, "Touch Keyboard and Handwriting. Touch keyboard, pen input and some IMEs break."),
    ("stisvc", I, "Windows Image Acquisition. Scanners and some cameras stop working."),
    ("wbiosrvc", I, "Windows Biometric Service. Fingerprint / face sign-in (Windows Hello) stops working."),
    ("bthserv", I, "Bluetooth Support. Bluetooth devices stop working."),
    ("bthavctpsvc", I, "AVCTP. Bluetooth audio controls stop working."),
    ("spooler", I, "Print Spooler. Printing and Print to PDF stop. Safe only if you never print."),
    ("wpnservice", I, "Windows Push Notifications. Toast notifications and live tiles stop."),
    ("camsvc", I, "Capability Access Manager. Manages app permissions (camera, mic, location)."),
    ("dusmsvc", I, "Data Usage. Metered connections and data limits."),
    ("displayenhancementservice", O, "Display Enhancement. Brightness/HDR helpers on some laptops."),
    ("deviceassociationservice", I, "Device Association. Pairing of wireless devices."),
    ("dsmsvc", I, "Device Setup Manager. Installing new hardware/drivers."),
    // Security
    ("windefend", C, "Microsoft Defender Antivirus. Disabling removes real-time malware protection."),
    ("wdnissvc", C, "Defender Network Inspection. Part of Microsoft Defender protection."),
    ("securityhealthservice", C, "Windows Security Service. Security center and protection status."),
    ("sgrmbroker", C, "System Guard Runtime Monitor. Platform integrity protection."),
    ("wscsvc", C, "Security Center. Reports antivirus/firewall health; protections may be reported off."),
    ("sense", C, "Defender for Endpoint. Corporate security monitoring."),
    ("mdcoresvc", C, "Microsoft Defender Core. Part of Microsoft Defender protection."),
    ("vaultsvc", I, "Credential Manager. Saved passwords for apps and network shares stop working."),
    ("ngcsvc", C, "Microsoft Passport. Windows Hello PIN sign-in depends on it."),
    ("ngcctnrsvc", C, "Microsoft Passport Container. Windows Hello PIN sign-in depends on it."),
    ("wlidsvc", I, "Microsoft Account Sign-in Assistant. Microsoft account sign-in for Windows and apps."),
    ("tokenbroker", I, "Web Account Manager. Sign-in for Store apps, Office and Microsoft accounts."),
    ("seclogon", O, "Secondary Logon. 'Run as different user'."),
    ("remoteregistry", O, "Remote Registry. Lets other PCs edit this registry; disabling improves security."),
    // Updates and Store
    ("wuauserv", I, "Windows Update. You will stop receiving security updates."),
    ("usosvc", I, "Update Orchestrator. Coordinates Windows Update; updates stop."),
    ("waasmedicsvc", I, "Windows Update Medic. Repairs Windows Update; protected by Windows."),
    ("bits", I, "Background Intelligent Transfer. Windows Update, Defender and Store downloads use it."),
    ("dosvc", I, "Delivery Optimization. Windows Update and Store downloads use it."),
    ("installservice", I, "Microsoft Store Install Service. Store apps cannot install or update."),
    ("appxsvc", I, "AppX Deployment. Store/UWP apps cannot install or update."),
    ("clipsvc", I, "Client License Service. Store apps may refuse to start."),
    ("licensemanager", I, "Windows License Manager. Store app licensing."),
    ("w32time", I, "Windows Time. Clock drifts; HTTPS, Kerberos and sign-in can fail when time is wrong."),
    // Performance / indexing
    ("sysmain", I, "SysMain (Superfetch). Preloads frequently used apps. Disabling can slow app launches on SSDs too."),
    ("wsearch", I, "Windows Search indexing. Start menu, Explorer and Outlook search become slow or incomplete."),
    ("trkwks", O, "Distributed Link Tracking. Fixes shortcuts to files moved across NTFS volumes."),
    ("pcasvc", O, "Program Compatibility Assistant. Detects and fixes old-app compatibility problems."),
    ("dps", O, "Diagnostic Policy Service. Windows troubleshooters and problem detection."),
    ("wdiservicehost", O, "Diagnostic Service Host. Windows troubleshooters."),
    ("wdisystemhost", O, "Diagnostic System Host. Windows troubleshooters."),
    // Telemetry and consumer features
    ("diagtrack", O, "Connected User Experiences and Telemetry. Sends diagnostic data to Microsoft."),
    ("dmwappushservice", O, "WAP Push Message Routing. Device-management messages and telemetry routing."),
    ("wersvc", O, "Windows Error Reporting. Sends crash reports to Microsoft."),
    ("mapsbroker", O, "Downloaded Maps Manager. Only for the Maps app."),
    ("lfsvc", O, "Geolocation. Location for apps; Find My Device needs it."),
    ("retaildemo", O, "Retail Demo. Only used by store display PCs."),
    ("wisvc", O, "Windows Insider Service. Only needed for Insider builds."),
    ("wmpnetworksvc", O, "Windows Media Player Network Sharing."),
    ("fax", O, "Fax."),
    ("phonesvc", O, "Phone Service. Telephony state for apps."),
    ("xblauthmanager", O, "Xbox Live Auth Manager. Only for Xbox apps and games."),
    ("xblgamesave", O, "Xbox Live Game Save. Cloud saves for Xbox games."),
    ("xboxnetapisvc", O, "Xbox Live Networking. Only for Xbox games."),
    ("xboxgipsvc", O, "Xbox Accessory Management. Xbox controllers' firmware and configuration."),
    ("cdpsvc", I, "Connected Devices Platform. Nearby sharing, Phone Link, timeline and some Bluetooth features."),
    ("onesyncsvc", O, "Sync Host. Syncs mail, contacts and calendar for built-in apps."),
    // Virtualization
    ("vmms", I, "Hyper-V Virtual Machine Management. Hyper-V VMs stop working."),
    ("vmcompute", I, "Hyper-V Host Compute. WSL2, Docker and Windows Sandbox need it."),
    ("hvhost", I, "Hyper-V Host Service. Hyper-V performance features."),
    ("lxssmanager", I, "WSL Service. Windows Subsystem for Linux stops working."),
    ("wslservice", I, "WSL Service. Windows Subsystem for Linux stops working."),
    // Common third-party
    ("edgeupdate", O, "Microsoft Edge updater. Edge will not update in the background."),
    ("edgeupdatem", O, "Microsoft Edge updater. Edge will not update in the background."),
    ("microsoftedgeelevationservice", O, "Edge elevation helper. Used by Edge updates/installs."),
    ("gupdate", O, "Google updater. Chrome will not update in the background."),
    ("gupdatem", O, "Google updater. Chrome will not update in the background."),
    ("mozillamaintenance", O, "Mozilla Maintenance. Firefox updates silently without UAC prompts."),
    ("adobearmservice", O, "Adobe Acrobat Update Service."),
    ("brave", O, "Brave updater."),
    ("bravem", O, "Brave updater."),
    ("steam client service", O, "Steam Client Service. Started by Steam when needed."),
    ("vmtools", I, "VMware Tools. Clipboard, display resizing and time sync inside a VMware VM."),
    ("vboxservice", I, "VirtualBox Guest Additions. Clipboard, display resizing and shared folders inside a VM."),
    ("clicktorunsvc", I, "Microsoft Office Click-to-Run. Office apps will not start or update without it."),
    ("ccmexec", I, "Configuration Manager client. Corporate software deployment and compliance."),
    ("intunemanagementextension", I, "Intune Management Extension. Corporate device management."),
];

/// Scheduled task folders / paths (lowercase). The longest matching prefix wins.
const TASKS: &[(&str, Criticality, &str)] = &[
    (r"\microsoft\windows\textservicesframework\", C, "Starts the text input framework. Keyboard, IME and touch keyboard can stop working."),
    (r"\microsoft\windows\windows defender\", I, "Microsoft Defender maintenance (scans, cleanup, verification)."),
    (r"\microsoft\windows\updateorchestrator\", I, "Windows Update scheduling. Updates may stop installing."),
    (r"\microsoft\windows\windowsupdate\", I, "Windows Update. Updates may stop installing."),
    (r"\microsoft\windows\waasmedic\", I, "Windows Update repair."),
    (r"\microsoft\windows\softwareprotectionplatform\", I, "Windows activation / licensing refresh."),
    (r"\microsoft\windows\bitlocker\", I, "BitLocker drive encryption."),
    (r"\microsoft\windows\systemrestore\", I, "Creates System Restore points."),
    (r"\microsoft\windows\certificateservicesclient\", I, "Certificate auto-enrollment (corporate Wi-Fi, VPN and sign-in)."),
    (r"\microsoft\windows\windowscolorsystem\", I, "Loads your display color (ICC) profile at sign-in."),
    (r"\microsoft\windows\management\provisioning\", I, "Device provisioning / management."),
    (r"\microsoft\windows\workplace join\", I, "Work or school account (Azure AD) device registration."),
    (r"\microsoft\windows\plug and play\", I, "Device installation maintenance."),
    (r"\microsoft\windows\time synchronization\", I, "Keeps the clock in sync."),
    (r"\microsoft\windows\clip\", I, "Store app license validation; apps may refuse to start."),
    (r"\microsoft\windows\bluetooth\", I, "Bluetooth device maintenance."),
    (r"\microsoft\windows\customer experience improvement program\", O, "Telemetry (CEIP). Safe to disable."),
    (r"\microsoft\windows\application experience\", O, "Compatibility appraisal and telemetry. Safe to disable."),
    (r"\microsoft\windows\feedback\", O, "Feedback / survey telemetry. Safe to disable."),
    (r"\microsoft\windows\windows error reporting\", O, "Uploads queued crash reports."),
    (r"\microsoft\windows\maps\", O, "Offline maps maintenance."),
    (r"\microsoft\windows\xblgamesave\", O, "Xbox game cloud save sync."),
    (r"\microsoft\windows\cloudexperiencehost\", O, "Out-of-box / account setup experience."),
    (r"\microsoft\windows\diskdiagnostic\", O, "Disk diagnostics telemetry."),
    (r"\microsoft\windows\autochk\", O, "Telemetry proxy for disk checks."),
    (r"\microsoft\windows\nettrace\", O, "Network diagnostics collection."),
    (r"\microsoft\windows\device information\", O, "Device census telemetry."),
    (r"\microsoft\windows\flighting\", O, "Windows Insider configuration."),
    (r"\microsoft\windows\pi\", O, "Telemetry (SQM)."),
    (r"\microsoft\windows\location\", O, "Location notifications."),
    (r"\microsoft\windows\retaildemo\", O, "Retail demo mode cleanup."),
    (r"\microsoft\windows\settingsync\", O, "Settings sync."),
    (r"\microsoft\windows\shell\familysafety", O, "Microsoft Family Safety monitoring. Only needed with a family account."),
    (r"\microsoft\windows\mobile broadband accounts\", O, "Mobile broadband (cellular) account setup."),
    (r"\microsoft\office\", O, "Microsoft Office background task (telemetry, updates or licensing heartbeat)."),
];

/// Run-key / Startup-folder entries matched by keyword in name, command or publisher.
/// (keyword, criticality, impact, reason)
const APPS: &[(&str, Criticality, Impact, &str)] = &[
    ("ctfmon", C, Impact::Low, "Text input (keyboard, IME, touch keyboard). Typing may stop working."),
    ("securityhealth", I, Impact::Low, "Windows Security tray icon. Protection keeps running, but you won't see security alerts."),
    ("windowsdefender", I, Impact::Low, "Windows Security tray icon. Protection keeps running, but you won't see security alerts."),
    ("vboxtray", I, Impact::Low, "VirtualBox guest integration (clipboard, display resize)."),
    ("vmware user process", I, Impact::Low, "VMware guest integration (clipboard, display resize)."),
    ("vmtoolsd", I, Impact::Low, "VMware guest integration (clipboard, display resize)."),
    ("steam", O, Impact::High, "Steam client. Games launch Steam when you start them."),
    ("discord", O, Impact::High, "Discord. Open it when you need it."),
    ("epicgameslauncher", O, Impact::High, "Epic Games Launcher."),
    ("eadesktop", O, Impact::High, "EA app."),
    ("ea desktop", O, Impact::High, "EA app."),
    ("battle.net", O, Impact::High, "Battle.net launcher."),
    ("ubisoft", O, Impact::High, "Ubisoft Connect."),
    ("upc.exe", O, Impact::High, "Ubisoft Connect."),
    ("gog galaxy", O, Impact::Medium, "GOG Galaxy."),
    ("msteams", O, Impact::High, "Microsoft Teams. You won't receive calls/messages until you open it."),
    ("teams", O, Impact::High, "Microsoft Teams. You won't receive calls/messages until you open it."),
    ("slack", O, Impact::High, "Slack. You won't receive messages until you open it."),
    ("skype", O, Impact::Medium, "Skype."),
    ("zoom", O, Impact::Medium, "Zoom."),
    ("whatsapp", O, Impact::Medium, "WhatsApp."),
    ("telegram", O, Impact::Low, "Telegram."),
    ("spotify", O, Impact::Medium, "Spotify."),
    ("onedrive", O, Impact::Medium, "OneDrive. Files won't sync until you open OneDrive."),
    ("dropbox", O, Impact::High, "Dropbox. Files won't sync until you open Dropbox."),
    ("googledrivefs", O, Impact::Medium, "Google Drive. Files won't sync until you open it."),
    ("google drive", O, Impact::Medium, "Google Drive. Files won't sync until you open it."),
    ("icloud", O, Impact::Medium, "iCloud. Files and photos won't sync until you open it."),
    ("megasync", O, Impact::Medium, "MEGA sync."),
    ("creative cloud", O, Impact::High, "Adobe Creative Cloud background processes."),
    ("ccxprocess", O, Impact::High, "Adobe Creative Cloud experience process."),
    ("adobegcclient", O, Impact::Medium, "Adobe Genuine Software integrity check."),
    ("coresync", O, Impact::Medium, "Adobe Creative Cloud file sync."),
    ("acrobat", O, Impact::Low, "Adobe Acrobat helper."),
    ("msedge", O, Impact::Medium, "Microsoft Edge startup boost. Edge just opens a bit slower without it."),
    ("chrome", O, Impact::Medium, "Google Chrome background mode."),
    ("opera.exe", O, Impact::Medium, "Opera browser."),
    ("opera gx", O, Impact::Medium, "Opera GX browser."),
    ("brave", O, Impact::Medium, "Brave browser."),
    ("firefox", O, Impact::Medium, "Firefox."),
    ("utorrent", O, Impact::Medium, "uTorrent."),
    ("bittorrent", O, Impact::Medium, "BitTorrent."),
    ("qbittorrent", O, Impact::Medium, "qBittorrent."),
    ("docker desktop", O, Impact::High, "Docker Desktop. Containers won't run until you open it."),
    ("lghub", O, Impact::Medium, "Logitech G HUB. Device profiles may fall back to defaults until opened."),
    ("razer", O, Impact::Medium, "Razer Synapse. Device lighting/profiles may fall back to defaults."),
    ("icue", O, Impact::Medium, "Corsair iCUE. Device lighting/profiles may fall back to defaults."),
    ("armoury", O, Impact::High, "ASUS Armoury Crate. Lighting/fan profiles may fall back to defaults."),
    ("wallpaper", O, Impact::Medium, "Wallpaper Engine."),
    ("ccleaner", O, Impact::Low, "CCleaner monitoring."),
    ("jusched", O, Impact::Low, "Java update checker."),
    ("ituneshelper", O, Impact::Low, "iTunes helper (device detection)."),
    ("cortana", O, Impact::Medium, "Cortana."),
    ("nordvpn", O, Impact::Medium, "VPN client; it won't auto-connect at sign-in."),
    ("expressvpn", O, Impact::Medium, "VPN client; it won't auto-connect at sign-in."),
    ("protonvpn", O, Impact::Medium, "VPN client; it won't auto-connect at sign-in."),
    ("surfshark", O, Impact::Medium, "VPN client; it won't auto-connect at sign-in."),
    ("teamviewer", O, Impact::Medium, "TeamViewer. Unattended remote access unavailable until opened."),
    ("anydesk", O, Impact::Low, "AnyDesk. Unattended remote access unavailable until opened."),
    ("igfxtray", O, Impact::Low, "Legacy Intel graphics tray icon."),
    ("hkcmd", O, Impact::Low, "Legacy Intel graphics hotkeys."),
    ("igfxpers", O, Impact::Low, "Legacy Intel graphics persistence module."),
];

const SECURITY_VENDORS: &[&str] = &[
    "kaspersky", "eset", "bitdefender", "avast", "avg technologies", "norton", "symantec", "mcafee", "sophos",
    "malwarebytes", "crowdstrike", "sentinelone", "trend micro", "webroot", "f-secure", "panda security", "cylance",
    "carbon black", "zonealarm", "comodo", "g data", "emsisoft", "watchguard", "cisco secure", "trellix",
];

const HARDWARE_VENDORS: &[&str] = &[
    "nvidia", "advanced micro devices", "amd ", "radeon", "intel", "realtek", "synaptics", "elan microelectronics",
    "alps", "conexant", "dolby", "dts", "waves", "maxxaudio", "thunderbolt", "bluetooth", "touchpad", "fingerprint",
    "goodix", "qualcomm", "broadcom", "killer", "mediatek", "cirrus logic", "hotkey", "fan control", "battery",
];

const UPDATER_HINTS: &[&str] = &["update", "updater", "maintenance service", "autoupdate", "telemetry", "crash reporter"];

fn windows_dir_lower() -> String {
    std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()).to_lowercase()
}

/// Strips the per-user suffix of per-user service instances: "cbdhsvc_4a3b2" -> "cbdhsvc".
fn base_service_name(name: &str) -> String {
    let lower = name.to_lowercase();
    match lower.rsplit_once('_') {
        Some((base, suffix)) if suffix.len() >= 4 && suffix.chars().all(|c| c.is_ascii_hexdigit()) => base.to_string(),
        _ => lower,
    }
}

fn is_windows_component(item: &StartupItem) -> bool {
    let in_windir = item
        .target
        .as_ref()
        .is_some_and(|t| t.to_string_lossy().to_lowercase().starts_with(&windows_dir_lower()));
    let ms_task = item.source == SourceKind::ScheduledTask && item.location.to_lowercase().starts_with(r"\microsoft\windows");
    (item.is_microsoft && in_windir) || ms_task
}

/// True when `kw` occurs in `hay` starting at a word boundary (and, if `whole`, ending at one too).
fn has_word(hay: &str, kw: &str, whole: bool) -> bool {
    let kw = kw.trim();
    let is_word = |c: char| c.is_ascii_alphanumeric();
    hay.match_indices(kw).any(|(i, _)| {
        let before_ok = hay[..i].chars().next_back().is_none_or(|c| !is_word(c));
        let after_ok = !whole || hay[i + kw.len()..].chars().next().is_none_or(|c| !is_word(c));
        before_ok && after_ok
    })
}

fn find_keyword<'a, T>(hay: &str, table: &'a [(&'a str, T)]) -> Option<&'a T> {
    table.iter().find(|(k, _)| has_word(hay, k, false)).map(|(_, v)| v)
}

pub fn classify(item: &mut StartupItem) {
    let hay = item.haystack();
    let publisher = item.publisher.clone().unwrap_or_default().to_lowercase();
    let windows_component = is_windows_component(item);
    let mut known_impact: Option<(Impact, &'static str)> = None;

    let (crit, reason): (Criticality, String) = 'decide: {
        if item.source == SourceKind::Winlogon {
            break 'decide (C, "Launched by Windows sign-in itself (desktop shell / user initialization).".into());
        }

        if let Some(svc) = &item.service {
            let base = base_service_name(&svc.name);
            if let Some((_, c, r)) = SERVICES.iter().find(|(n, ..)| *n == base) {
                break 'decide (*c, (*r).into());
            }
        }

        if item.source == SourceKind::ScheduledTask {
            let path = item.facts.iter().find(|(k, _)| k == "Task path").map(|(_, v)| v.to_lowercase()).unwrap_or_default();
            if let Some((_, c, r)) = TASKS.iter().filter(|(p, ..)| path.starts_with(p)).max_by_key(|(p, ..)| p.len()) {
                break 'decide (*c, (*r).into());
            }
        }

        if matches!(item.source, SourceKind::RegistryRun | SourceKind::StartupFolder | SourceKind::ScheduledTask) {
            let table: Vec<(&str, (Criticality, Impact, &str))> = APPS.iter().map(|(k, c, i, r)| (*k, (*c, *i, *r))).collect();
            // Match on name/command first so a Microsoft-signed app (Teams, OneDrive) isn't treated as a system part.
            if let Some((c, i, r)) = find_keyword(&hay, &table) {
                if item.source != SourceKind::ScheduledTask || !windows_component {
                    known_impact = Some((*i, r));
                    break 'decide (*c, (*r).into());
                }
            }
        }

        if SECURITY_VENDORS.iter().any(|v| has_word(&publisher, v, true) || has_word(&hay, v, true)) {
            break 'decide if item.source == SourceKind::Service {
                (C, "Security software. Disabling it may leave this PC unprotected (its self-protection may also block the change).".into())
            } else {
                (I, "Part of your security software (tray/agent). Protection may be reduced or alerts hidden.".into())
            };
        }

        if !item.target_exists && item.target.is_some() && !item.command.starts_with("COM handler") {
            break 'decide (O, "The program this entry points to no longer exists. It's a leftover from an uninstalled app and safe to disable.".into());
        }

        if !windows_component && UPDATER_HINTS.iter().any(|h| hay.contains(h)) {
            break 'decide (O, "Updater / telemetry helper. The app keeps working; it just won't update or report in the background.".into());
        }

        if !windows_component && HARDWARE_VENDORS.iter().any(|v| has_word(&publisher, v, true) || has_word(&hay, v, true)) {
            break 'decide (
                I,
                "Hardware vendor component (graphics, audio, touchpad, Bluetooth, hotkeys…). Disabling can remove control panels, \
                 hotkeys, audio effects or device features."
                    .into(),
            );
        }

        // Microsoft-published services outside Windows (Office, Edge…) still deserve caution;
        // Microsoft apps launched at sign-in (PowerToys, Teams) are ordinary apps.
        let system_side = windows_component || (item.source == SourceKind::Service && item.is_microsoft);
        match (item.source, system_side) {
            (SourceKind::Service, true) => (
                I,
                "Windows service that isn't in Wield's knowledge base. Leave it on unless you know what it does; \
                 set it to Manual rather than Disabled if you experiment."
                    .into(),
            ),
            (SourceKind::Service, false) => (
                O,
                "Third-party service. Windows does not depend on it, but the app that installed it may lose background features.".into(),
            ),
            (SourceKind::ScheduledTask, true) => {
                (I, "Windows maintenance task. Usually harmless to leave on; disabling may affect the related Windows feature.".into())
            }
            (SourceKind::ScheduledTask, false) => {
                (O, "Not part of Windows. Windows does not depend on this task; the owning app may lose a background feature.".into())
            }
            (_, true) => (I, "Windows component launched at sign-in.".into()),
            (_, false) => (O, "Not part of Windows. Windows does not need it to start; you can open the app yourself when needed.".into()),
        }
    };
    item.criticality = crit;
    item.criticality_reason = reason;

    let (impact, why) = estimate_impact(item, known_impact);
    item.impact = impact;
    item.impact_reason = why;
}

fn estimate_impact(item: &StartupItem, known: Option<(Impact, &str)>) -> (Impact, String) {
    if item.source == SourceKind::Winlogon {
        return (Impact::Low, "Part of Windows sign-in.".into());
    }
    let mut level = Impact::Low;
    let mut reasons: Vec<String> = Vec::new();

    if let Some((i, _)) = known {
        level = level.max(i);
        reasons.push(format!("Known {}-impact app.", i.label().to_lowercase()));
    }

    if let Some(mem) = item.memory {
        let by_mem = if mem >= 300 << 20 {
            Impact::High
        } else if mem >= 80 << 20 {
            Impact::Medium
        } else {
            Impact::Low
        };
        level = level.max(by_mem);
        reasons.push(format!("Using {} of RAM right now.", format_bytes(mem)));
    }

    let shared_svchost = item.service.as_ref().is_some_and(|s| s.shared_process);
    if let Some(size) = item.file_size.filter(|_| !shared_svchost) {
        let by_size = if size >= 60 << 20 {
            Impact::High
        } else if size >= 10 << 20 {
            Impact::Medium
        } else {
            Impact::Low
        };
        // Size is a weak signal when we already measured memory.
        if item.memory.is_none() {
            level = level.max(by_size);
        }
        reasons.push(format!("Program file is {}.", format_bytes(size)));
    }

    if let Some(svc) = &item.service {
        if shared_svchost {
            reasons.push("Runs inside a shared Windows service host (svchost), so its own cost is small.".into());
        } else if svc.start_type == 2 && !svc.delayed && !item.is_microsoft {
            level = level.max(Impact::Medium);
            reasons.push("Third-party service that starts during boot.".into());
        }
    }

    let delayed = item.service.as_ref().is_some_and(|s| s.delayed) || item.facts.iter().any(|(k, _)| k == "Delayed");
    if delayed {
        level = level.lower();
        reasons.push("Starts with a delay, after the desktop is ready.".into());
    }

    if !item.target_exists && item.target.is_some() {
        return (Impact::Low, "Target file is missing, so nothing actually starts.".into());
    }
    if reasons.is_empty() {
        reasons.push("Small footprint.".into());
    }
    (level, format!("Estimated. {}", reasons.join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::startup::model::{Locator, ServiceDetails};

    fn run_item(name: &str, cmd: &str) -> StartupItem {
        StartupItem::new("x".into(), name.into(), cmd.into(), SourceKind::RegistryRun, "HKCU".into(), Locator::ReadOnly)
    }

    #[test]
    fn steam_is_optional_and_heavy() {
        let mut it = run_item("Steam", r#""C:\Program Files (x86)\Steam\steam.exe" -silent"#);
        it.target_exists = true;
        classify(&mut it);
        assert_eq!(it.criticality, Criticality::Optional);
        assert_eq!(it.impact, Impact::High);
    }

    #[test]
    fn rpcss_is_critical() {
        let mut it = StartupItem::new("s".into(), "RPC".into(), "svchost".into(), SourceKind::Service, "RpcSs".into(), Locator::ReadOnly);
        it.service = Some(ServiceDetails { name: "RpcSs".into(), start_type: 2, delayed: false, shared_process: true });
        classify(&mut it);
        assert_eq!(it.criticality, Criticality::Critical);
    }

    #[test]
    fn per_user_suffix_stripped() {
        assert_eq!(base_service_name("CDPUserSvc_4a3b2c"), "cdpusersvc");
        assert_eq!(base_service_name("Steam Client Service"), "steam client service");
    }

    #[test]
    fn word_boundaries() {
        assert!(has_word(r"c:\program files\eset\egui.exe", "eset", true));
        assert!(!has_word("resetpassword task", "eset", true));
        assert!(!has_word("intelligent search", "intel", true));
        assert!(has_word(r"c:\steam\steamwebhelper.exe", "steam", false));
    }

    #[test]
    fn missing_target_is_safe() {
        let mut it = run_item("Old thing", r"C:\gone\old.exe");
        it.target = Some(r"C:\gone\old.exe".into());
        it.target_exists = false;
        classify(&mut it);
        assert_eq!(it.criticality, Criticality::Optional);
    }
}
