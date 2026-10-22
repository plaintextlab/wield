//! Windows' own keyboard shortcuts, and how (or whether) each one can be turned off.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cat {
    Essentials,
    WinKey,
    Windows,
    Screenshots,
    Accessibility,
    Language,
    Explorer,
    Text,
    Taskbar,
}

impl Cat {
    pub const ALL: [Cat; 9] = [Cat::Essentials, Cat::WinKey, Cat::Windows, Cat::Screenshots, Cat::Accessibility, Cat::Language, Cat::Explorer, Cat::Taskbar, Cat::Text];
    pub fn label(self) -> &'static str {
        match self {
            Cat::Essentials => "Essentials",
            Cat::WinKey => "Windows key",
            Cat::Windows => "Windows, snapping & desktops",
            Cat::Screenshots => "Screenshots & recording",
            Cat::Accessibility => "Accessibility",
            Cat::Language => "Language & input",
            Cat::Explorer => "File Explorer",
            Cat::Text => "Text editing",
            Cat::Taskbar => "Taskbar & Start",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Both,
    Win10,
    Win11,
}

/// A setting that turns one or more shortcuts on or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Toggle {
    /// Every Win+ shortcut (NoWinKeys policy).
    AllWinKeys,
    StickyKeys,
    FilterKeys,
    ToggleKeys,
    HighContrast,
    MouseKeys,
    ColorFilters,
    Narrator,
    PrintScreenSnip,
    LanguageAltShift,
    LayoutCtrlShift,
    Snap,
    Shake,
    Lock,
}

impl Toggle {
    pub fn label(self) -> &'static str {
        match self {
            Toggle::AllWinKeys => "All Windows-key shortcuts",
            Toggle::StickyKeys => "Sticky Keys shortcut",
            Toggle::FilterKeys => "Filter Keys shortcut",
            Toggle::ToggleKeys => "Toggle Keys shortcut",
            Toggle::HighContrast => "High contrast shortcut",
            Toggle::MouseKeys => "Mouse Keys shortcut",
            Toggle::ColorFilters => "Color filters shortcut",
            Toggle::Narrator => "Narrator shortcut",
            Toggle::PrintScreenSnip => "Print Screen opens screen snipping",
            Toggle::LanguageAltShift => "Switch input language",
            Toggle::LayoutCtrlShift => "Switch keyboard layout",
            Toggle::Snap => "Snap windows",
            Toggle::Shake => "Minimize other windows (shake)",
            Toggle::Lock => "Lock the PC",
        }
    }
    /// What has to happen before a change is noticed.
    pub fn applies(self) -> &'static str {
        match self {
            Toggle::StickyKeys | Toggle::FilterKeys | Toggle::ToggleKeys | Toggle::HighContrast | Toggle::MouseKeys | Toggle::Snap | Toggle::Lock | Toggle::Narrator | Toggle::ColorFilters => "Applies right away.",
            Toggle::Shake | Toggle::AllWinKeys => "Applies after Explorer restarts.",
            Toggle::PrintScreenSnip | Toggle::LanguageAltShift | Toggle::LayoutCtrlShift => "Applies after you sign out and back in.",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// Can't be turned off; the text says why and what to do instead.
    Fixed(&'static str),
    /// Win+<these keys>, turned off through Explorer's DisabledHotkeys list.
    WinKeys(&'static str),
    Toggle(Toggle),
}

#[derive(Clone, Copy, Debug)]
pub struct Builtin {
    pub keys: &'static str,
    pub action: &'static str,
    pub cat: Cat,
    pub os: Os,
    pub control: Control,
}

const APP: Control = Control::Fixed("Built into apps themselves, so there's no Windows setting for it. Some apps let you change it in their own settings.");
const SHELL: Control = Control::Fixed("Handled directly by the Windows shell; Windows has no setting to turn it off. PowerToys Keyboard Manager can remap it.");
const SECURE: Control = Control::Fixed("Reserved by Windows for security. No program can change or intercept it.");
const DESKTOPS: Control = Control::Fixed("Part of Task View and virtual desktops; Windows has no setting to turn it off. PowerToys Keyboard Manager can remap it.");
const GAMEBAR: Control = Control::Fixed("Change or clear it in Xbox Game Bar: press Win+G → Settings → Shortcuts.");

macro_rules! b {
    ($keys:literal, $action:literal, $cat:ident, $os:ident, $control:expr) => {
        Builtin { keys: $keys, action: $action, cat: Cat::$cat, os: Os::$os, control: $control }
    };
}

use Control::{Toggle as T, WinKeys as W};

pub const BUILTIN: &[Builtin] = &[
    // Essentials
    b!("Ctrl+Alt+Delete", "Security screen: lock, switch user, sign out, Task Manager", Essentials, Both, SECURE),
    b!("Ctrl+Shift+Esc", "Open Task Manager", Essentials, Both, Control::Fixed("Can only be removed by blocking Task Manager completely, which Wield doesn't do. Process Explorer is an alternative.")),
    b!("Alt+Tab", "Switch between open windows", Essentials, Both, SHELL),
    b!("Ctrl+Alt+Tab", "Show open windows until you pick one", Essentials, Both, SHELL),
    b!("Alt+Shift+Tab", "Switch between open windows, backwards", Essentials, Both, SHELL),
    b!("Ctrl+Alt+Shift+Tab", "Show open windows until you pick one, backwards", Essentials, Both, SHELL),
    b!("Alt+F4", "Close the active window (on the desktop: shut down dialog)", Essentials, Both, SHELL),
    b!("Win+L", "Lock the PC", Essentials, Both, T(Toggle::Lock)),
    b!("Win+D", "Show or hide the desktop", Essentials, Both, W("D")),
    b!("Win+E", "Open File Explorer", Essentials, Both, W("E")),
    b!("Win+R", "Open the Run box", Essentials, Both, W("R")),
    b!("Win+I", "Open Settings", Essentials, Both, W("I")),
    b!("Win+X", "Quick link menu (Device Manager, Terminal, Disk Management…)", Essentials, Both, W("X")),
    b!("Win+V", "Clipboard history", Essentials, Both, W("V")),
    b!("Win+.", "Emoji, symbols and GIF panel", Essentials, Both, SHELL),
    b!("Win+Ctrl+Shift+B", "Restart the graphics driver (fixes a black or frozen screen)", Essentials, Both, Control::Fixed("A recovery shortcut built into the graphics system; it can't be turned off.")),
    // Windows key
    b!("Win", "Open or close Start", WinKey, Both, Control::Fixed("Windows has no setting for the Windows key alone. PowerToys Keyboard Manager can remap or disable it.")),
    b!("Win+S", "Search", WinKey, Both, W("S")),
    b!("Win+Q", "Search", WinKey, Both, W("Q")),
    b!("Win+A", "Action center (Windows 10) / Quick settings (Windows 11)", WinKey, Both, W("A")),
    b!("Win+N", "Notification center and calendar", WinKey, Win11, W("N")),
    b!("Win+B", "Focus the notification area (system tray)", WinKey, Both, W("B")),
    b!("Win+T", "Cycle through taskbar apps", WinKey, Both, W("T")),
    b!("Win+U", "Accessibility settings", WinKey, Both, W("U")),
    b!("Win+H", "Voice typing (dictation)", WinKey, Both, W("H")),
    b!("Win+K", "Cast to a wireless display or audio device", WinKey, Both, W("K")),
    b!("Win+P", "Projection mode (duplicate, extend, second screen only)", WinKey, Both, W("P")),
    b!("Win+C", "Copilot (Windows 11) / Cortana (Windows 10)", WinKey, Both, W("C")),
    b!("Win+W", "Widgets (Windows 11) / Windows Ink Workspace (Windows 10)", WinKey, Both, W("W")),
    b!("Win+F", "Feedback Hub", WinKey, Both, W("F")),
    b!("Win+O", "Lock screen rotation (tablets)", WinKey, Both, W("O")),
    b!("Win+J", "Jump to the Windows tip, when shown", WinKey, Both, W("J")),
    b!("Win+Y", "Switch input between Windows Mixed Reality and the desktop", WinKey, Win10, W("Y")),
    b!("Win+Alt+D", "Show date and time on the taskbar", WinKey, Both, SHELL),
    b!("Win+Ctrl+Q", "Quick Assist (remote help)", WinKey, Both, SHELL),
    b!("Win+Alt+K", "Mute or unmute the microphone in calls", WinKey, Win11, SHELL),
    b!("Win+Pause", "System information in Settings", WinKey, Both, SHELL),
    // Windows, snapping & desktops
    b!("Win+Up", "Maximize the window", Windows, Both, T(Toggle::Snap)),
    b!("Win+Down", "Restore or minimize the window", Windows, Both, T(Toggle::Snap)),
    b!("Win+Left", "Snap the window to the left half", Windows, Both, T(Toggle::Snap)),
    b!("Win+Right", "Snap the window to the right half", Windows, Both, T(Toggle::Snap)),
    b!("Win+Shift+Up", "Stretch the window to the top and bottom of the screen", Windows, Both, T(Toggle::Snap)),
    b!("Win+Z", "Snap layouts", Windows, Win11, W("Z")),
    b!("Win+Home", "Minimize every window except the active one", Windows, Both, T(Toggle::Shake)),
    b!("Win+M", "Minimize all windows", Windows, Both, W("M")),
    b!("Win+Shift+M", "Restore minimized windows", Windows, Both, W("M")),
    b!("Win+Shift+Left", "Move the window to the monitor on the left", Windows, Both, SHELL),
    b!("Win+Shift+Right", "Move the window to the monitor on the right", Windows, Both, SHELL),
    b!("Win+Tab", "Task View: all windows and virtual desktops", Windows, Both, DESKTOPS),
    b!("Win+Ctrl+D", "Add a virtual desktop", Windows, Both, DESKTOPS),
    b!("Win+Ctrl+Left", "Switch to the virtual desktop on the left", Windows, Both, DESKTOPS),
    b!("Win+Ctrl+Right", "Switch to the virtual desktop on the right", Windows, Both, DESKTOPS),
    b!("Win+Ctrl+F4", "Close the current virtual desktop", Windows, Both, DESKTOPS),
    b!("Win+,", "Peek at the desktop", Windows, Both, SHELL),
    b!("Alt+Space", "Window menu (restore, move, size, close)", Windows, Both, SHELL),
    b!("Alt+Esc", "Cycle through windows in the order they were opened", Windows, Both, SHELL),
    // Screenshots & recording
    b!("PrtScn", "Open screen snipping (when on), otherwise copy the screen", Screenshots, Both, T(Toggle::PrintScreenSnip)),
    b!("Win+Shift+S", "Screen snipping: rectangle, window or full screen", Screenshots, Both, W("S")),
    b!("Alt+PrtScn", "Copy the active window to the clipboard", Screenshots, Both, SHELL),
    b!("Win+PrtScn", "Save a full screenshot to Pictures\\Screenshots", Screenshots, Both, SHELL),
    b!("Win+Shift+R", "Record a screen video with Snipping Tool", Screenshots, Win11, SHELL),
    b!("Win+G", "Open Xbox Game Bar", Screenshots, Both, W("G")),
    b!("Win+Alt+R", "Start or stop recording a game clip", Screenshots, Both, GAMEBAR),
    b!("Win+Alt+G", "Record the last 30 seconds (Game Bar)", Screenshots, Both, GAMEBAR),
    b!("Win+Alt+PrtScn", "Screenshot of the game (Game Bar)", Screenshots, Both, GAMEBAR),
    b!("Win+Alt+B", "Turn HDR on or off", Screenshots, Both, GAMEBAR),
    // Accessibility
    b!("Shift five times", "Turn Sticky Keys on or off", Accessibility, Both, T(Toggle::StickyKeys)),
    b!("Hold Right Shift 8 seconds", "Turn Filter Keys on or off", Accessibility, Both, T(Toggle::FilterKeys)),
    b!("Hold Num Lock 5 seconds", "Turn Toggle Keys on or off", Accessibility, Both, T(Toggle::ToggleKeys)),
    b!("Left Alt+Left Shift+PrtScn", "Turn high contrast on or off", Accessibility, Both, T(Toggle::HighContrast)),
    b!("Left Alt+Left Shift+Num Lock", "Turn Mouse Keys on or off", Accessibility, Both, T(Toggle::MouseKeys)),
    b!("Win+Ctrl+C", "Turn color filters on or off", Accessibility, Both, T(Toggle::ColorFilters)),
    b!("Win+Ctrl+Enter", "Turn Narrator on or off", Accessibility, Both, T(Toggle::Narrator)),
    b!("Win+Plus", "Magnifier: zoom in (starts Magnifier)", Accessibility, Both, SHELL),
    b!("Win+Minus", "Magnifier: zoom out", Accessibility, Both, SHELL),
    b!("Win+Esc", "Close Magnifier", Accessibility, Both, SHELL),
    b!("Win+Ctrl+O", "On-screen keyboard", Accessibility, Both, SHELL),
    b!("Win+Ctrl+N", "Narrator settings", Accessibility, Both, SHELL),
    // Language & input
    b!("Left Alt+Shift", "Switch input language", Language, Both, T(Toggle::LanguageAltShift)),
    b!("Ctrl+Shift", "Switch keyboard layout within a language", Language, Both, T(Toggle::LayoutCtrlShift)),
    b!("Win+Space", "Switch input language and keyboard layout", Language, Both, Control::Fixed("Always on in Windows 10 and 11. Remove extra keyboard layouts in Settings → Time & language if you don't need them.")),
    b!("Win+;", "Emoji panel", Language, Both, SHELL),
    // File Explorer
    b!("Ctrl+N", "New window", Explorer, Both, APP),
    b!("Ctrl+T", "New tab", Explorer, Win11, APP),
    b!("Ctrl+W", "Close the window or tab", Explorer, Both, APP),
    b!("Ctrl+Shift+N", "New folder", Explorer, Both, APP),
    b!("F2", "Rename", Explorer, Both, APP),
    b!("Ctrl+F", "Search in the folder", Explorer, Both, APP),
    b!("Ctrl+L", "Go to the address bar", Explorer, Both, APP),
    b!("Alt+Up", "Up one folder", Explorer, Both, APP),
    b!("Alt+Left", "Back", Explorer, Both, APP),
    b!("Alt+Right", "Forward", Explorer, Both, APP),
    b!("Alt+Enter", "Properties", Explorer, Both, APP),
    b!("Alt+P", "Show the preview pane", Explorer, Both, APP),
    b!("Alt+Shift+P", "Show the details pane", Explorer, Win11, APP),
    b!("Shift+Delete", "Delete without the Recycle Bin", Explorer, Both, APP),
    b!("Ctrl+Shift+C", "Copy as path", Explorer, Win11, APP),
    b!("F5", "Refresh", Explorer, Both, APP),
    b!("F11", "Full screen", Explorer, Both, APP),
    // Taskbar & Start
    b!("Win+1", "Open or switch to taskbar app 1 (Win+2… for the others)", Taskbar, Both, W("1234567890")),
    b!("Win+Alt+1", "Jump list of taskbar app 1", Taskbar, Both, W("1234567890")),
    b!("Win+Ctrl+1", "Switch to the last window of taskbar app 1", Taskbar, Both, W("1234567890")),
    b!("Shift+click a taskbar app", "Open another window of the app", Taskbar, Both, SHELL),
    b!("Ctrl+Shift+click a taskbar app", "Open the app as administrator", Taskbar, Both, SHELL),
    b!("Ctrl+Esc", "Open Start", Taskbar, Both, SHELL),
    // Text editing
    b!("Ctrl+C", "Copy", Text, Both, APP),
    b!("Ctrl+X", "Cut", Text, Both, APP),
    b!("Ctrl+V", "Paste", Text, Both, APP),
    b!("Ctrl+Z", "Undo", Text, Both, APP),
    b!("Ctrl+Y", "Redo", Text, Both, APP),
    b!("Ctrl+A", "Select all", Text, Both, APP),
    b!("Ctrl+Backspace", "Delete the previous word", Text, Both, APP),
    b!("Ctrl+Left", "Move to the previous word", Text, Both, APP),
    b!("Ctrl+Right", "Move to the next word", Text, Both, APP),
    b!("Ctrl+Home", "Go to the start", Text, Both, APP),
    b!("Ctrl+End", "Go to the end", Text, Both, APP),
    b!("Shift+F10", "Open the right-click menu", Text, Both, APP),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::hotkeys::keys::Combo;

    #[test]
    fn catalog_is_sane() {
        for b in BUILTIN {
            if let Control::WinKeys(k) = b.control {
                assert!(!k.is_empty() && k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()), "{}", b.keys);
                assert!(b.keys.starts_with("Win+"), "{}", b.keys);
            }
        }
        // Real key combinations must parse, so they can be matched against hotkeys in use.
        for k in ["Win+E", "Win+Shift+S", "Ctrl+Alt+Delete", "Win+Ctrl+Left", "Win+.", "Win+,", "Win+;", "PrtScn", "Alt+PrtScn", "Win+Plus"] {
            assert!(Combo::parse(k).is_some(), "{k}");
        }
    }
}
