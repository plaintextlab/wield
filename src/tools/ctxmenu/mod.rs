//! Context Menu Manager: every right-click menu entry, hide/delete/restore, backups and your own entries.
pub mod reg;
pub mod scan;
pub mod edit;
pub mod custom;
pub mod ui;

#[cfg(test)]
/// Tests that change the real registry run one at a time.
pub static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
