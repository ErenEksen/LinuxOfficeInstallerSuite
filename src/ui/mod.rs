pub mod bridge;

/// Guard for installer-ui "No gradients": no gradient brush anywhere in `ui/**/*.slint`, and
/// only checked-safe items imported from Slint's std-widgets.
#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// std-widgets imports reviewed to paint no gradient (Palette only provides colors).
    const ALLOWED_STD_WIDGETS: &[&str] = &["Palette"];

    fn slint_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                slint_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "slint") {
                out.push(p);
            }
        }
    }

    fn strip_comments(src: &str) -> String {
        let mut out = String::new();
        let mut in_block = false;
        for line in src.lines() {
            let mut l = line.to_string();
            if in_block {
                match l.find("*/") {
                    Some(i) => {
                        l = l[i + 2..].to_string();
                        in_block = false;
                    }
                    None => continue,
                }
            }
            if let Some(i) = l.find("/*") {
                in_block = !l[i..].contains("*/");
                l.truncate(i);
            }
            if let Some(i) = l.find("//") {
                l.truncate(i);
            }
            out.push_str(&l);
            out.push('\n');
        }
        out
    }

    pub fn violations(root: &Path) -> Vec<String> {
        let mut files = Vec::new();
        slint_files(root, &mut files);
        let mut bad = Vec::new();
        for f in files {
            let code = strip_comments(&std::fs::read_to_string(&f).unwrap());
            for (n, line) in code.lines().enumerate() {
                if line.to_ascii_lowercase().contains("gradient") {
                    bad.push(format!("{}:{}: {}", f.display(), n + 1, line.trim()));
                }
                if line.contains("std-widgets.slint")
                    && let (Some(a), Some(b)) = (line.find('{'), line.find('}'))
                {
                    for item in line[a + 1..b].split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        if !ALLOWED_STD_WIDGETS.contains(&item) {
                            bad.push(format!("{}:{}: unchecked std-widgets import {item}", f.display(), n + 1));
                        }
                    }
                }
            }
        }
        bad
    }

    #[test]
    fn no_gradients_in_ui() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let bad = violations(&root);
        assert!(bad.is_empty(), "gradient constructs or unchecked std-widgets imports:\n{}", bad.join("\n"));
    }

    #[test]
    fn guard_catches_a_gradient() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("x.slint"),
            "// gradients are mentioned in comments only\nimport { Button, Palette } from \"std-widgets.slint\";\n\
             component X inherits Rectangle { background: @linear-gradient(90deg, #fff 0%, #000 100%); }\n",
        )
        .unwrap();
        let bad = violations(tmp.path());
        assert_eq!(bad.len(), 2, "{bad:?}");
        assert!(bad.iter().any(|b| b.contains("@linear-gradient")));
        assert!(bad.iter().any(|b| b.contains("unchecked std-widgets import Button")));
    }
}
