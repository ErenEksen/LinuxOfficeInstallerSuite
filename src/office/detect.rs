//! Installed Office app detection (office-installation: Installed app detection) and the
//! per-app facts the launchers need.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum App {
    Word,
    Excel,
    PowerPoint,
    Outlook,
    OneNote,
    Access,
    Publisher,
}

impl App {
    pub const ALL: [App; 7] =
        [App::Word, App::Excel, App::PowerPoint, App::Outlook, App::OneNote, App::Access, App::Publisher];

    /// Wrapper argument and file-name suffix: `msoffice-<id>.desktop`.
    pub fn id(self) -> &'static str {
        match self {
            App::Word => "word",
            App::Excel => "excel",
            App::PowerPoint => "powerpoint",
            App::Outlook => "outlook",
            App::OneNote => "onenote",
            App::Access => "access",
            App::Publisher => "publisher",
        }
    }

    pub fn from_id(id: &str) -> Option<App> {
        App::ALL.into_iter().find(|a| a.id() == id)
    }

    pub fn exe(self) -> &'static str {
        match self {
            App::Word => "WINWORD.EXE",
            App::Excel => "EXCEL.EXE",
            App::PowerPoint => "POWERPNT.EXE",
            App::Outlook => "OUTLOOK.EXE",
            App::OneNote => "ONENOTE.EXE",
            App::Access => "MSACCESS.EXE",
            App::Publisher => "MSPUB.EXE",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            App::Word => "Microsoft Word",
            App::Excel => "Microsoft Excel",
            App::PowerPoint => "Microsoft PowerPoint",
            App::Outlook => "Microsoft Outlook",
            App::OneNote => "Microsoft OneNote",
            App::Access => "Microsoft Access",
            App::Publisher => "Microsoft Publisher",
        }
    }

    pub fn generic_name(self) -> &'static str {
        match self {
            App::Word => "Word Processor",
            App::Excel => "Spreadsheet",
            App::PowerPoint => "Presentation",
            App::Outlook => "Email Client",
            App::OneNote => "Note Taking",
            App::Access => "Database",
            App::Publisher => "Desktop Publishing",
        }
    }

    /// Extra desktop categories after `Office;`.
    pub fn categories(self) -> &'static str {
        match self {
            App::Word => "Office;WordProcessor;",
            App::Excel => "Office;Spreadsheet;",
            App::PowerPoint => "Office;Presentation;",
            App::Outlook => "Office;Email;",
            App::OneNote => "Office;",
            App::Access => "Office;Database;",
            App::Publisher => "Office;Publishing;",
        }
    }

    /// MIME types for the formats in the desktop-integration spec.
    pub fn mime_types(self) -> &'static [&'static str] {
        match self {
            App::Word => &[
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document", // docx
                "application/vnd.ms-word.document.macroEnabled.12",                        // docm
                "application/vnd.openxmlformats-officedocument.wordprocessingml.template", // dotx
                "application/vnd.ms-word.template.macroEnabled.12",                        // dotm
                "application/msword",                                                      // doc
                "application/msword-template",                                             // dot
                "application/rtf",                                                         // rtf
                "text/rtf",
                "application/vnd.oasis.opendocument.text", // odt
            ],
            App::Excel => &[
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",    // xlsx
                "application/vnd.ms-excel.sheet.macroEnabled.12",                       // xlsm
                "application/vnd.openxmlformats-officedocument.spreadsheetml.template", // xltx
                "application/vnd.ms-excel.template.macroEnabled.12",                    // xltm
                "application/vnd.ms-excel.sheet.binary.macroEnabled.12",                // xlsb
                "application/vnd.ms-excel",                                             // xls, xlt
                "text/csv",                                                             // csv
                "application/vnd.oasis.opendocument.spreadsheet",                       // ods
            ],
            App::PowerPoint => &[
                "application/vnd.openxmlformats-officedocument.presentationml.presentation", // pptx
                "application/vnd.ms-powerpoint.presentation.macroEnabled.12",                // pptm
                "application/vnd.openxmlformats-officedocument.presentationml.template",     // potx
                "application/vnd.ms-powerpoint.template.macroEnabled.12",                    // potm
                "application/vnd.openxmlformats-officedocument.presentationml.slideshow",    // ppsx
                "application/vnd.ms-powerpoint.slideshow.macroEnabled.12",                   // ppsm
                "application/vnd.ms-powerpoint",                                             // ppt, pot, pps
                "application/vnd.oasis.opendocument.presentation",                           // odp
            ],
            App::Outlook => &["application/vnd.ms-outlook", "message/rfc822"],
            App::OneNote => &["application/onenote"],
            App::Access => &["application/x-msaccess", "application/vnd.ms-access"],
            App::Publisher => &["application/vnd.ms-publisher", "application/x-mspublisher"],
        }
    }
}

/// `Office16` folders to search: 64-bit Office lives in `Program Files`, 32-bit Office
/// in `Program Files (x86)`.
pub fn office16_candidates(prefix: &Path) -> [PathBuf; 2] {
    let c = prefix.join("drive_c");
    [
        c.join("Program Files/Microsoft Office/root/Office16"),
        c.join("Program Files (x86)/Microsoft Office/root/Office16"),
    ]
}

/// The `Office16` folder that holds an installation, if any.
pub fn office16_dir(prefix: &Path) -> Option<PathBuf> {
    office16_candidates(prefix).into_iter().find(|d| !scan(d).is_empty())
}

/// Windows path of an `Office16` folder found by [`office16_dir`].
pub fn office16_windows_path(dir: &Path) -> &'static str {
    if dir.to_string_lossy().contains("Program Files (x86)") {
        r"C:\Program Files (x86)\Microsoft Office\root\Office16"
    } else {
        r"C:\Program Files\Microsoft Office\root\Office16"
    }
}

/// Find the app EXE in `dir`, matching case-insensitively.
pub fn find_exe(dir: &Path, app: App) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(app.exe())))
}

fn scan(dir: &Path) -> Vec<App> {
    App::ALL.into_iter().filter(|a| find_exe(dir, *a).is_some()).collect()
}

/// Installed apps, in menu order.
pub fn detect(prefix: &Path) -> Vec<App> {
    office16_dir(prefix).map(|d| scan(&d)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(prefix: &Path, sub: &str, exes: &[&str]) {
        let dir = prefix.join("drive_c").join(sub).join("Microsoft Office/root/Office16");
        std::fs::create_dir_all(&dir).unwrap();
        for e in exes {
            std::fs::write(dir.join(e), b"MZ").unwrap();
        }
        std::fs::write(dir.join("MSO.DLL"), b"MZ").unwrap();
    }

    #[test]
    fn full_install() {
        let tmp = tempfile::tempdir().unwrap();
        fake(tmp.path(), "Program Files", &App::ALL.map(|a| a.exe()));
        assert_eq!(detect(tmp.path()), App::ALL.to_vec());
    }

    #[test]
    fn partial_install_without_access_and_publisher() {
        let tmp = tempfile::tempdir().unwrap();
        fake(tmp.path(), "Program Files", &["WINWORD.EXE", "excel.exe", "POWERPNT.EXE", "OUTLOOK.EXE", "ONENOTE.EXE"]);
        assert_eq!(detect(tmp.path()), [App::Word, App::Excel, App::PowerPoint, App::Outlook, App::OneNote]);
        let dir = office16_dir(tmp.path()).unwrap();
        assert_eq!(office16_windows_path(&dir), r"C:\Program Files\Microsoft Office\root\Office16");
    }

    #[test]
    fn thirty_two_bit_and_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(detect(tmp.path()).is_empty());
        fake(tmp.path(), "Program Files (x86)", &["WINWORD.EXE"]);
        assert_eq!(detect(tmp.path()), [App::Word]);
        assert!(office16_windows_path(&office16_dir(tmp.path()).unwrap()).contains("(x86)"));
    }

    #[test]
    fn mime_lists_cover_the_spec() {
        assert!(App::Word.mime_types().contains(&"application/vnd.oasis.opendocument.text"));
        assert!(App::Excel.mime_types().contains(&"text/csv"));
        assert!(App::PowerPoint.mime_types().contains(&"application/vnd.oasis.opendocument.presentation"));
        for a in App::ALL {
            assert_eq!(App::from_id(a.id()), Some(a));
            assert!(!a.mime_types().is_empty());
        }
    }
}
