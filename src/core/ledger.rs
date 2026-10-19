//! Record of every file written outside `~/.msoffice/` and every MIME default changed,
//! so removal deletes exactly those and nothing else (desktop-integration: Recorded file ledger).

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::config::write_atomic;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    /// Office launchers and their icons.
    Office,
    /// The app's own binary, icon and desktop entry (self-install).
    App,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: PathBuf,
    pub category: Category,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MimeEntry {
    pub mime: String,
    /// Desktop file id that was the default before we changed it (`None` = no default).
    pub previous: Option<String>,
    /// Desktop file id we set.
    pub ours: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    files: Vec<FileEntry>,
    #[serde(default)]
    mime_defaults: Vec<MimeEntry>,
    #[serde(skip)]
    path: PathBuf,
}

impl Ledger {
    /// Load the ledger; a missing or unreadable file starts empty.
    pub fn load(path: &Path) -> Self {
        let mut ledger: Ledger = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| match toml::from_str(&t) {
                Ok(l) => Some(l),
                Err(e) => {
                    log::warn!("ignoring unreadable ledger {}: {e}", path.display());
                    None
                }
            })
            .unwrap_or_default();
        ledger.path = path.to_path_buf();
        ledger
    }

    pub fn save(&self) -> io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
        write_atomic(&self.path, text.as_bytes())
    }

    /// Record a file we wrote. Saves immediately so a crash can't lose the entry.
    pub fn record_file(&mut self, path: &Path, category: Category) -> io::Result<()> {
        if !self.files.iter().any(|f| f.path == path) {
            self.files.push(FileEntry { path: path.to_path_buf(), category });
        }
        self.save()
    }

    /// Record a MIME default change. If the type was already changed by us, the original
    /// previous default is kept, so a restore goes back to what the user had.
    pub fn record_mime(&mut self, mime: &str, previous: Option<String>, ours: &str) -> io::Result<()> {
        match self.mime_defaults.iter_mut().find(|m| m.mime == mime) {
            Some(entry) => entry.ours = ours.to_string(),
            None => self.mime_defaults.push(MimeEntry { mime: mime.into(), previous, ours: ours.into() }),
        }
        self.save()
    }

    #[cfg(test)]
    pub fn files(&self, category: Category) -> impl Iterator<Item = &FileEntry> {
        self.files.iter().filter(move |f| f.category == category)
    }

    pub fn mime_defaults(&self) -> &[MimeEntry] {
        &self.mime_defaults
    }

    /// Delete every recorded file of a category and drop the entries. Files that are
    /// already gone are fine. Returns the deleted paths.
    pub fn remove_category(&mut self, category: Category) -> io::Result<Vec<PathBuf>> {
        let mut removed = Vec::new();
        let mut kept = Vec::new();
        let mut first_err = None;
        for entry in std::mem::take(&mut self.files) {
            if entry.category != category {
                kept.push(entry);
                continue;
            }
            match std::fs::remove_file(&entry.path) {
                Ok(()) => removed.push(entry.path),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    first_err.get_or_insert(e);
                    kept.push(entry);
                }
            }
        }
        self.files = kept;
        self.save()?;
        match first_err {
            Some(e) => Err(e),
            None => Ok(removed),
        }
    }

    /// Forget recorded MIME defaults (after they were restored). Returns them.
    pub fn take_mime_defaults(&mut self) -> io::Result<Vec<MimeEntry>> {
        let taken = std::mem::take(&mut self.mime_defaults);
        self.save()?;
        Ok(taken)
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.mime_defaults.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removal_deletes_exactly_the_recorded_files() {
        let tmp = tempfile::tempdir().unwrap();
        let apps = tmp.path().join("applications");
        std::fs::create_dir_all(&apps).unwrap();
        let ours = apps.join("msoffice-word.desktop");
        let ours_icon = apps.join("msoffice-word.png");
        let app_entry = apps.join("linux-office-installer-suite.desktop");
        let foreign = apps.join("firefox.desktop");
        for f in [&ours, &ours_icon, &app_entry, &foreign] {
            std::fs::write(f, "x").unwrap();
        }

        let ledger_path = tmp.path().join("cfg/ledger.toml");
        let mut ledger = Ledger::load(&ledger_path);
        ledger.record_file(&ours, Category::Office).unwrap();
        ledger.record_file(&ours_icon, Category::Office).unwrap();
        ledger.record_file(&ours, Category::Office).unwrap(); // duplicate is ignored
        ledger.record_file(&app_entry, Category::App).unwrap();
        ledger.record_file(&apps.join("already-gone.desktop"), Category::Office).unwrap();

        // Entries persist across loads.
        let mut ledger = Ledger::load(&ledger_path);
        assert_eq!(ledger.files(Category::Office).count(), 3);

        let mut removed = ledger.remove_category(Category::Office).unwrap();
        removed.sort();
        assert_eq!(removed, vec![ours.clone(), ours_icon.clone()]);
        assert!(!ours.exists() && !ours_icon.exists());
        assert!(foreign.exists(), "unrecorded file must be untouched");
        assert!(app_entry.exists(), "other category must be untouched");

        let ledger = Ledger::load(&ledger_path);
        assert_eq!(ledger.files(Category::Office).count(), 0);
        assert_eq!(ledger.files(Category::App).count(), 1);
    }

    #[test]
    fn mime_keeps_the_original_previous_default() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ledger.toml");
        let mut ledger = Ledger::load(&path);
        ledger.record_mime("text/rtf", Some("libreoffice-writer.desktop".into()), "msoffice-word.desktop").unwrap();
        ledger.record_mime("text/rtf", Some("msoffice-word.desktop".into()), "msoffice-word.desktop").unwrap();
        ledger.record_mime("application/msword", None, "msoffice-word.desktop").unwrap();
        let mut ledger = Ledger::load(&path);
        assert_eq!(ledger.mime_defaults().len(), 2);
        assert_eq!(ledger.mime_defaults()[0].previous.as_deref(), Some("libreoffice-writer.desktop"));
        let taken = ledger.take_mime_defaults().unwrap();
        assert_eq!(taken.len(), 2);
        assert!(Ledger::load(&path).is_empty());
    }
}
