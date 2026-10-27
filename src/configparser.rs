//! The part of Python's `configparser.ConfigParser` brainglobe-atlasapi uses:
//! reading INI text (sections, `key = value` / `key: value`, comments,
//! continuation lines, lower-cased keys), and writing it back as Python does
//! (`key = value`, a blank line after each section).

use crate::error::{Error, Result};

/// `configparser.DEFAULTSECT`.
pub const DEFAULTSECT: &str = "DEFAULT";

/// An ordered INI document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConfigParser {
    defaults: Vec<(String, String)>,
    sections: Vec<(String, Vec<(String, String)>)>,
}

impl ConfigParser {
    pub fn new() -> ConfigParser {
        ConfigParser::default()
    }

    /// `read_string`: parse INI text, adding to what is already here.
    pub fn read_string(&mut self, text: &str) -> Result<()> {
        // (section index or None for DEFAULT, key) of the value a continuation
        // line extends
        let mut cur: Option<Option<usize>> = None;
        let mut last_key: Option<String> = None;
        let mut indent_level = 0usize;
        for (lineno, raw) in text.lines().enumerate() {
            let line = raw.trim_end_matches('\r');
            let stripped = line.trim();
            // full-line comments (inline comments are off by default)
            if stripped.starts_with('#') || stripped.starts_with(';') {
                continue;
            }
            let indent = line.len() - line.trim_start().len();
            if stripped.is_empty() {
                // an empty line ends a multi-line value (empty_lines_in_values
                // keeps it, then trailing blanks are stripped on read)
                if let (Some(sect), Some(key)) = (cur, last_key.as_ref())
                    && let Some(v) = self.value_mut(sect, key)
                {
                    v.push('\n');
                }
                continue;
            }
            if let (Some(sect), Some(key)) = (cur, last_key.as_ref())
                && indent > indent_level
            {
                if let Some(v) = self.value_mut(sect, key) {
                    v.push('\n');
                    v.push_str(stripped);
                }
                continue;
            }
            indent_level = indent;
            if stripped.starts_with('[') && stripped.ends_with(']') {
                let name = &stripped[1..stripped.len() - 1];
                if name == DEFAULTSECT {
                    cur = Some(None);
                } else {
                    let idx = match self.sections.iter().position(|(n, _)| n == name) {
                        Some(i) => i,
                        None => {
                            self.sections.push((name.to_string(), vec![]));
                            self.sections.len() - 1
                        }
                    };
                    cur = Some(Some(idx));
                }
                last_key = None;
                continue;
            }
            let Some(sect) = cur else {
                return Err(Error::value(format!(
                    "File contains no section headers.\nfile: '<string>', line: {}\n{}",
                    lineno + 1,
                    crate::utils::py_repr_str(&format!("{raw}\n"))
                )));
            };
            let pos = stripped.find(['=', ':']);
            let (key, value) = match pos {
                Some(p) => (stripped[..p].trim(), stripped[p + 1..].trim()),
                None => (stripped, ""),
            };
            let key = key.to_lowercase();
            self.set_in(sect, &key, value);
            last_key = Some(key);
        }
        // values lose the blank lines that ended them
        for (_, kv) in self.sections.iter_mut() {
            for (_, v) in kv.iter_mut() {
                *v = v.trim_end_matches('\n').to_string();
            }
        }
        for (_, v) in self.defaults.iter_mut() {
            *v = v.trim_end_matches('\n').to_string();
        }
        Ok(())
    }

    /// `read(path)`: a missing file is skipped, as in Python.
    pub fn read(&mut self, path: &std::path::Path) -> Result<()> {
        match std::fs::read_to_string(path) {
            Ok(t) => self.read_string(&t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn value_mut(&mut self, sect: Option<usize>, key: &str) -> Option<&mut String> {
        let kv = match sect {
            None => &mut self.defaults,
            Some(i) => &mut self.sections[i].1,
        };
        kv.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn set_in(&mut self, sect: Option<usize>, key: &str, value: &str) {
        let kv = match sect {
            None => &mut self.defaults,
            Some(i) => &mut self.sections[i].1,
        };
        match kv.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value.to_string(),
            None => kv.push((key.to_string(), value.to_string())),
        }
    }

    /// The section names, without DEFAULT (`sections()`).
    pub fn sections(&self) -> Vec<String> {
        self.sections.iter().map(|(n, _)| n.clone()).collect()
    }

    pub fn has_section(&self, section: &str) -> bool {
        self.sections.iter().any(|(n, _)| n == section)
    }

    /// `conf[section] = {..}`: replace (or add) a whole section.
    pub fn set_section(&mut self, section: &str, items: Vec<(String, String)>) {
        let items = items
            .into_iter()
            .map(|(k, v)| (k.to_lowercase(), v))
            .collect();
        if section == DEFAULTSECT {
            self.defaults = items;
            return;
        }
        match self.sections.iter_mut().find(|(n, _)| n == section) {
            Some((_, kv)) => *kv = items,
            None => self.sections.push((section.to_string(), items)),
        }
    }

    /// `conf[section][key]`, falling back on DEFAULT.
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        let key = key.to_lowercase();
        if section != DEFAULTSECT {
            let (_, kv) = self.sections.iter().find(|(n, _)| n == section)?;
            if let Some((_, v)) = kv.iter().find(|(k, _)| *k == key) {
                return Some(v);
            }
        }
        self.defaults
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }

    /// `conf[section][key] = value` (the section must exist).
    pub fn set(&mut self, section: &str, key: &str, value: &str) -> Result<()> {
        let key = key.to_lowercase();
        if section == DEFAULTSECT {
            self.set_in(None, &key, value);
            return Ok(());
        }
        let idx = self
            .sections
            .iter()
            .position(|(n, _)| n == section)
            .ok_or_else(|| Error::key(section))?;
        self.set_in(Some(idx), &key, value);
        Ok(())
    }

    /// `conf.items()` as Python iterates it: DEFAULT first, then every
    /// section; each section's items include the defaults it inherits (its
    /// own keys first, then the defaults it does not override).
    pub fn items(&self) -> Vec<(String, Vec<(String, String)>)> {
        let mut out = vec![(DEFAULTSECT.to_string(), self.defaults.clone())];
        for (name, kv) in &self.sections {
            let mut all = kv.clone();
            for (k, v) in &self.defaults {
                if !all.iter().any(|(kk, _)| kk == k) {
                    all.push((k.clone(), v.clone()));
                }
            }
            out.push((name.clone(), all));
        }
        out
    }

    /// One section's own items plus inherited defaults (`dict(conf[section])`).
    pub fn section_items(&self, section: &str) -> Option<Vec<(String, String)>> {
        self.items()
            .into_iter()
            .find(|(n, _)| n == section)
            .map(|(_, kv)| kv)
    }

    /// `conf.write(f)`: the text Python writes.
    pub fn to_ini_string(&self) -> String {
        let mut s = String::new();
        let mut section = |name: &str, kv: &[(String, String)]| {
            s.push_str(&format!("[{name}]\n"));
            for (k, v) in kv {
                let v = v.replace('\n', "\n\t");
                if v.is_empty() {
                    s.push_str(&format!("{k} = \n"));
                } else {
                    s.push_str(&format!("{k} = {v}\n"));
                }
            }
            s.push('\n');
        };
        if !self.defaults.is_empty() {
            section(DEFAULTSECT, &self.defaults);
        }
        for (name, kv) in &self.sections {
            section(name, kv);
        }
        s
    }

    /// Write the INI text to `path`.
    pub fn write(&self, path: &std::path::Path) -> Result<()> {
        std::fs::write(path, self.to_ini_string())?;
        Ok(())
    }
}
