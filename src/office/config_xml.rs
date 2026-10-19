//! Typed deployment form model, rendered to the ODT `Configuration.xml` (design D9).

use serde::{Deserialize, Serialize};

use super::plans::{Channel, Plan};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Edition {
    #[default]
    #[serde(rename = "64")]
    X64,
    #[serde(rename = "32")]
    X86,
}

impl Edition {
    pub fn odt_value(self) -> &'static str {
        match self {
            Edition::X64 => "64",
            Edition::X86 => "32",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OfficeApp {
    Word,
    Excel,
    PowerPoint,
    Outlook,
    OneNote,
    Access,
    Publisher,
    OneDrive,
    Teams,
    Skype,
}

impl OfficeApp {
    /// Form order.
    pub const ALL: [OfficeApp; 10] = [
        OfficeApp::Word,
        OfficeApp::Excel,
        OfficeApp::PowerPoint,
        OfficeApp::Outlook,
        OfficeApp::OneNote,
        OfficeApp::Access,
        OfficeApp::Publisher,
        OfficeApp::OneDrive,
        OfficeApp::Teams,
        OfficeApp::Skype,
    ];

    pub fn label(self) -> &'static str {
        match self {
            OfficeApp::Word => "Word",
            OfficeApp::Excel => "Excel",
            OfficeApp::PowerPoint => "PowerPoint",
            OfficeApp::Outlook => "Outlook",
            OfficeApp::OneNote => "OneNote",
            OfficeApp::Access => "Access",
            OfficeApp::Publisher => "Publisher",
            OfficeApp::OneDrive => "OneDrive",
            OfficeApp::Teams => "Teams",
            OfficeApp::Skype => "Skype for Business",
        }
    }

    /// The ID used by `<ExcludeApp>`.
    pub fn exclude_id(self) -> &'static str {
        match self {
            OfficeApp::Word => "Word",
            OfficeApp::Excel => "Excel",
            OfficeApp::PowerPoint => "PowerPoint",
            OfficeApp::Outlook => "Outlook",
            OfficeApp::OneNote => "OneNote",
            OfficeApp::Access => "Access",
            OfficeApp::Publisher => "Publisher",
            OfficeApp::OneDrive => "Groove",
            OfficeApp::Teams => "Teams",
            OfficeApp::Skype => "Lync",
        }
    }

    pub fn default_selected(self) -> bool {
        !matches!(self, OfficeApp::OneDrive | OfficeApp::Teams | OfficeApp::Skype)
    }

    /// Short note shown next to apps with known problems under Wine (bottlesdevs/programs#500).
    pub fn known_issue(self) -> Option<&'static str> {
        match self {
            OfficeApp::Outlook => Some("Known issues: classic Outlook may not start or sync under Wine."),
            OfficeApp::PowerPoint => Some("Known issues: adding a new slide can crash PowerPoint."),
            OfficeApp::OneNote => Some("Known issues: OneNote sync and rendering are unreliable."),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayLevel {
    #[default]
    None,
    Full,
}

impl DisplayLevel {
    pub fn odt_value(self) -> &'static str {
        match self {
            DisplayLevel::None => "None",
            DisplayLevel::Full => "Full",
        }
    }
}

/// Everything the Install form controls. Saved in config.toml and rendered to XML.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeploymentConfig {
    pub plan: Plan,
    pub edition: Edition,
    pub channel: Channel,
    /// Apps to install; every other app becomes an `<ExcludeApp>`.
    pub apps: Vec<OfficeApp>,
    /// Use `MatchOS` (with `en-us` fallback and explicit `en-us`).
    pub match_os: bool,
    /// Explicit language tags, used when `match_os` is false.
    pub languages: Vec<String>,
    pub updates_enabled: bool,
    pub display_level: DisplayLevel,
    pub logging: bool,
}

impl Default for DeploymentConfig {
    fn default() -> Self {
        Self {
            plan: Plan::default(),
            edition: Edition::default(),
            channel: Channel::default(),
            apps: OfficeApp::ALL.into_iter().filter(|a| a.default_selected()).collect(),
            match_os: true,
            languages: vec!["en-us".into()],
            updates_enabled: false,
            display_level: DisplayLevel::None,
            logging: true,
        }
    }
}

impl DeploymentConfig {
    pub fn includes(&self, app: OfficeApp) -> bool {
        self.apps.contains(&app)
    }

    pub fn set_app(&mut self, app: OfficeApp, on: bool) {
        self.apps.retain(|a| *a != app);
        if on {
            self.apps.push(app);
            self.apps.sort();
        }
    }
}

/// A pre-downloaded Office source for `setup.exe /configure`.
#[derive(Clone, Copy, Debug)]
pub struct InstallSource<'a> {
    /// Windows path of the folder that holds `Office\Data`.
    pub path: &'a str,
    pub version: &'a str,
}

/// Fixed log folder inside the prefix (polled for progress, design D10).
pub const LOG_DIR_WINDOWS: &str = r"C:\msoffice-logs";
pub const LOG_DIR_IN_DRIVE_C: &str = "msoffice-logs";

impl DeploymentConfig {
    /// Explicit language tags in output order.
    fn language_elements(&self) -> Vec<(String, Option<&'static str>)> {
        if self.match_os {
            return vec![("MatchOS".into(), Some("en-us")), ("en-us".into(), None)];
        }
        let mut langs: Vec<String> = Vec::new();
        for l in &self.languages {
            let l = l.trim().to_ascii_lowercase();
            if !l.is_empty() && !langs.contains(&l) {
                langs.push(l);
            }
        }
        if langs.is_empty() {
            langs.push("en-us".into());
        }
        langs.into_iter().map(|l| (l, None)).collect()
    }

    /// Excluded app IDs, sorted so the output order is fixed.
    pub fn excluded_ids(&self) -> Vec<&'static str> {
        let mut ids: Vec<&'static str> =
            OfficeApp::ALL.into_iter().filter(|a| !self.includes(*a)).map(|a| a.exclude_id()).collect();
        ids.sort_unstable();
        ids
    }

    /// Render `Configuration.xml` as shown in the preview.
    pub fn render(&self) -> String {
        self.render_with(None)
    }

    /// Render `Configuration.xml`. The installed file is the preview plus, when the Office
    /// files were downloaded beforehand, `SourcePath`, `Version` and `AllowCdnFallback` on
    /// `<Add>` (office-installation: Local Office source).
    pub fn render_with(&self, source: Option<InstallSource>) -> String {
        use quick_xml::Writer;
        let mut w = Writer::new_with_indent(Vec::new(), b' ', 2);
        w.config_mut().add_space_before_slash_in_empty_elements = true;
        let channel = self.plan.effective_channel(self.channel).odt_name();
        let result: std::io::Result<()> = (|| {
            w.create_element("Configuration").write_inner_content(|w| {
                let mut add = w
                    .create_element("Add")
                    .with_attribute(("OfficeClientEdition", self.edition.odt_value()))
                    .with_attribute(("Channel", channel));
                if let Some(s) = source {
                    add = add
                        .with_attribute(("SourcePath", s.path))
                        .with_attribute(("Version", s.version))
                        .with_attribute(("AllowCdnFallback", "TRUE"));
                }
                add.write_inner_content(|w| {
                    w.create_element("Product").with_attribute(("ID", self.plan.product_id())).write_inner_content(
                        |w| {
                            for (id, fallback) in self.language_elements() {
                                let mut e = w.create_element("Language").with_attribute(("ID", id.as_str()));
                                if let Some(f) = fallback {
                                    e = e.with_attribute(("Fallback", f));
                                }
                                e.write_empty()?;
                            }
                            for id in self.excluded_ids() {
                                w.create_element("ExcludeApp").with_attribute(("ID", id)).write_empty()?;
                            }
                            Ok(())
                        },
                    )?;
                    Ok(())
                })?;
                w.create_element("Display")
                    .with_attribute(("Level", self.display_level.odt_value()))
                    .with_attribute(("AcceptEULA", "TRUE"))
                    .write_empty()?;
                let updates = w
                    .create_element("Updates")
                    .with_attribute(("Enabled", if self.updates_enabled { "TRUE" } else { "FALSE" }));
                if self.updates_enabled {
                    updates.with_attribute(("Channel", channel)).write_empty()?;
                } else {
                    updates.write_empty()?;
                }
                if self.logging {
                    w.create_element("Logging")
                        .with_attribute(("Level", "Standard"))
                        .with_attribute(("Path", LOG_DIR_WINDOWS))
                        .write_empty()?;
                } else {
                    w.create_element("Logging").with_attribute(("Level", "Off")).write_empty()?;
                }
                Ok(())
            })?;
            Ok(())
        })();
        result.expect("writing XML to memory cannot fail");
        let mut s = String::from_utf8(w.into_inner()).expect("XML is UTF-8");
        s.push('\n');
        s
    }
}

/// The edition of an Office installation already in the prefix, read from the
/// `ClickToRun\Configuration` `Platform` value in `system.reg` (no Wine process needed).
pub fn installed_edition(system_reg: &str) -> Option<Edition> {
    let mut in_key = false;
    for line in system_reg.lines() {
        if line.starts_with('[') {
            in_key = line.to_ascii_lowercase().starts_with(r"[software\\microsoft\\office\\clicktorun\\configuration]");
            continue;
        }
        if in_key && let Some(v) = line.strip_prefix("\"Platform\"=") {
            return match v.trim().trim_matches('"').to_ascii_lowercase().as_str() {
                "x64" => Some(Edition::X64),
                "x86" => Some(Edition::X86),
                _ => None,
            };
        }
    }
    None
}

pub fn installed_edition_in(prefix: &std::path::Path) -> Option<Edition> {
    std::fs::read_to_string(prefix.join("system.reg")).ok().and_then(|t| installed_edition(&t))
}

/// Block a configuration whose architecture differs from the Office already installed.
pub fn check_architecture(cfg: &DeploymentConfig, installed: Option<Edition>) -> Result<(), String> {
    match installed {
        Some(e) if e != cfg.edition => Err(format!(
            "Office is already installed in this prefix as {}-bit. Mixed 32-bit and 64-bit installations are not \
             supported. Choose {}-bit, or use Reset prefix in Settings to start over.",
            e.odt_value(),
            e.odt_value()
        )),
        _ => Ok(()),
    }
}

/// Write the rendered XML to `~/.msoffice/Configuration.xml` and `C:\Configuration.xml` in
/// the prefix, from the same bytes. Returns the Windows path passed to setup.
pub fn write_config(paths: &crate::core::paths::Paths, xml: &str) -> std::io::Result<&'static str> {
    crate::core::config::write_atomic(&paths.configuration_xml(), xml.as_bytes())?;
    crate::core::config::write_atomic(&paths.prefix().join("drive_c/Configuration.xml"), xml.as_bytes())?;
    Ok(r"C:\Configuration.xml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn golden(name: &str) -> String {
        std::fs::read_to_string(format!("{}/tests/golden/{name}.xml", env!("CARGO_MANIFEST_DIR")))
            .unwrap_or_else(|_| panic!("missing golden file {name}.xml"))
    }

    /// Set `LOIS_BLESS=1` to (re)write golden files from the current output.
    fn check(name: &str, cfg: &DeploymentConfig) {
        let xml = cfg.render();
        if std::env::var_os("LOIS_BLESS").is_some() {
            let dir = format!("{}/tests/golden", env!("CARGO_MANIFEST_DIR"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(format!("{dir}/{name}.xml"), &xml).unwrap();
        }
        assert_eq!(xml, golden(name), "{name}");
        // Well-formed: quick-xml reads it back without errors.
        let mut r = quick_xml::Reader::from_str(&xml);
        loop {
            match r.read_event().unwrap() {
                quick_xml::events::Event::Eof => break,
                _ => {}
            }
        }
        for forbidden in ["PIDKEY", "AUTOACTIVATE", "KMS", "Property"] {
            assert!(!xml.to_ascii_uppercase().contains(&forbidden.to_ascii_uppercase()), "{name} contains {forbidden}");
        }
    }

    #[test]
    fn default_matches_design_example() {
        let xml = DeploymentConfig::default().render();
        assert_eq!(
            xml,
            r#"<Configuration>
  <Add OfficeClientEdition="64" Channel="Current">
    <Product ID="O365HomePremRetail">
      <Language ID="MatchOS" Fallback="en-us" />
      <Language ID="en-us" />
      <ExcludeApp ID="Groove" />
      <ExcludeApp ID="Lync" />
      <ExcludeApp ID="Teams" />
    </Product>
  </Add>
  <Display Level="None" AcceptEULA="TRUE" />
  <Updates Enabled="FALSE" />
  <Logging Level="Standard" Path="C:\msoffice-logs" />
</Configuration>
"#
        );
        check("default", &DeploymentConfig::default());
    }

    #[test]
    fn every_plan() {
        for plan in Plan::ALL {
            let cfg = DeploymentConfig { plan, channel: Channel::SemiAnnual, ..Default::default() };
            let xml = cfg.render();
            assert!(xml.contains(&format!("<Product ID=\"{}\">", plan.product_id())));
            let want = if plan.is_retail_2024() { "Current" } else { "SemiAnnual" };
            assert!(xml.contains(&format!("Channel=\"{want}\"")), "{plan:?}");
            check(&format!("plan-{}", plan.product_id()), &cfg);
        }
    }

    #[test]
    fn every_excluded_app() {
        for app in OfficeApp::ALL {
            let mut cfg = DeploymentConfig::default();
            for a in OfficeApp::ALL {
                cfg.set_app(a, a != app);
            }
            let xml = cfg.render();
            assert_eq!(xml.matches("<ExcludeApp").count(), 1);
            assert!(xml.contains(&format!("<ExcludeApp ID=\"{}\" />", app.exclude_id())), "{app:?}");
            check(&format!("exclude-{}", app.exclude_id()), &cfg);
        }
        let mut cfg = DeploymentConfig::default();
        cfg.set_app(OfficeApp::OneDrive, false);
        assert!(cfg.render().contains("<ExcludeApp ID=\"Groove\" />"));
    }

    #[test]
    fn advanced_and_languages() {
        let cfg = DeploymentConfig {
            edition: Edition::X86,
            match_os: false,
            languages: vec!["de-DE".into(), "en-us".into(), "de-de".into()],
            updates_enabled: true,
            display_level: DisplayLevel::Full,
            logging: false,
            ..Default::default()
        };
        let xml = cfg.render();
        assert!(xml.contains("OfficeClientEdition=\"32\""));
        assert!(xml.contains("<Language ID=\"de-de\" />\n      <Language ID=\"en-us\" />"));
        assert!(!xml.contains("MatchOS"));
        assert!(xml.contains("<Updates Enabled=\"TRUE\" Channel=\"Current\" />"));
        assert!(xml.contains("<Display Level=\"Full\" AcceptEULA=\"TRUE\" />"));
        assert!(xml.contains("<Logging Level=\"Off\" />"));
        check("advanced", &cfg);
    }

    #[test]
    fn local_source_attributes() {
        let cfg = DeploymentConfig::default();
        let src = InstallSource { path: r"Z:\home\u\.msoffice\cache\office-source\Current", version: "16.0.20430.20182" };
        let xml = cfg.render_with(Some(src));
        assert!(xml.contains(
            r#"<Add OfficeClientEdition="64" Channel="Current" SourcePath="Z:\home\u\.msoffice\cache\office-source\Current" Version="16.0.20430.20182" AllowCdnFallback="TRUE">"#
        ));
        // Apart from those attributes, the installed file is the preview.
        let stripped = xml.replace(
            r#" SourcePath="Z:\home\u\.msoffice\cache\office-source\Current" Version="16.0.20430.20182" AllowCdnFallback="TRUE""#,
            "",
        );
        assert_eq!(stripped, cfg.render());
    }

    #[test]
    fn architecture_guard() {
        let reg = "WINE REGISTRY Version 2\n\n[Software\\\\Microsoft\\\\Office\\\\ClickToRun\\\\Configuration] 1792418703\n#time=1dc\n\"CDNBaseUrl\"=\"http://officecdn.microsoft.com/pr/x\"\n\"Platform\"=\"x64\"\n\n[Software\\\\Other]\n\"Platform\"=\"x86\"\n";
        assert_eq!(installed_edition(reg), Some(Edition::X64));
        assert_eq!(installed_edition("WINE REGISTRY Version 2\n"), None);
        let x86 = DeploymentConfig { edition: Edition::X86, ..Default::default() };
        let err = check_architecture(&x86, installed_edition(reg)).unwrap_err();
        assert!(err.contains("Mixed 32-bit and 64-bit"));
        assert!(check_architecture(&DeploymentConfig::default(), installed_edition(reg)).is_ok());
        assert!(check_architecture(&x86, None).is_ok());
    }

    #[test]
    fn written_files_are_byte_identical_to_preview() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = crate::core::paths::Paths::for_home(tmp.path());
        let cfg = DeploymentConfig { plan: Plan::HomeBusiness2024, ..Default::default() };
        let preview = cfg.render();
        let win = write_config(&paths, &cfg.render()).unwrap();
        assert_eq!(win, r"C:\Configuration.xml");
        assert_eq!(std::fs::read(paths.configuration_xml()).unwrap(), preview.as_bytes());
        assert_eq!(std::fs::read(paths.prefix().join("drive_c/Configuration.xml")).unwrap(), preview.as_bytes());
    }
}
