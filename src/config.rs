//! Reading and modifying the BrainGlobe configuration
//! (`brainglobe_atlasapi.config`).
//!
//! The configuration is stored in a file, by default in
//! `$HOME/.config/brainglobe/bg_config.conf`; the environment variable
//! `BRAINGLOBE_CONFIG_DIR` overrides the directory. Python reads these once,
//! at import; here they are read when called.

use std::path::{Path, PathBuf};

use crate::configparser::ConfigParser;
use crate::error::Result;

pub const CONFIG_FILENAME: &str = "bg_config.conf";

/// `Path.home()`: `$HOME`.
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(std::env::home_dir)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// `CONFIG_DEFAULT_DIR`: `~/.config/brainglobe`.
pub fn config_default_dir() -> PathBuf {
    home().join(".config").join("brainglobe")
}

/// `CONFIG_DIR`: `$BRAINGLOBE_CONFIG_DIR`, else the default.
pub fn config_dir() -> PathBuf {
    std::env::var_os("BRAINGLOBE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(config_default_dir)
}

/// `CONFIG_PATH`.
pub fn config_path() -> PathBuf {
    config_dir().join(CONFIG_FILENAME)
}

/// `DEFAULT_PATH`: `~/.brainglobe`.
pub fn default_path() -> PathBuf {
    home().join(".brainglobe")
}

/// `DEFAULT_WORKDIR`: `~/brainglobe_workingdir`.
pub fn default_workdir() -> PathBuf {
    home().join("brainglobe_workingdir")
}

/// INI sections: (name, [(key, value)]).
pub type Sections = Vec<(String, Vec<(String, String)>)>;

/// `TEMPLATE_CONF_DICT`: section -> (key, value).
pub fn template_conf_dict() -> Sections {
    let p = default_path().to_string_lossy().to_string();
    vec![(
        "default_dirs".into(),
        vec![
            ("brainglobe_dir".into(), p.clone()),
            ("interm_download_dir".into(), p),
        ],
    )]
}

/// Write the configuration file at first use (`write_default_config`).
pub fn write_default_config(path: Option<&Path>, template: Option<Sections>) -> Result<()> {
    let path = path.map(Path::to_path_buf).unwrap_or_else(config_path);
    let template = template.unwrap_or_else(template_conf_dict);
    let mut conf = ConfigParser::new();
    for (k, val) in template {
        conf.set_section(&k, val);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    conf.write(&path)
}

/// Read the BrainGlobe config, writing the default one first if there is
/// none (`read_config`).
///
/// As in Python, a missing file is replaced by the default config written at
/// `CONFIG_PATH`, whatever `path` was asked for.
pub fn read_config(path: Option<&Path>) -> Result<ConfigParser> {
    let path = path.map(Path::to_path_buf).unwrap_or_else(config_path);
    if !path.exists() {
        write_default_config(None, None)?;
    }
    let mut conf = ConfigParser::new();
    conf.read(&path)?;
    Ok(conf)
}

/// Write a new value in the config file (`write_config_value`): every
/// section that has `key` gets `val`.
pub fn write_config_value(key: &str, val: &str, path: Option<&Path>) -> Result<()> {
    let path = path.map(Path::to_path_buf).unwrap_or_else(config_path);
    let mut conf = ConfigParser::new();
    conf.read(&path)?;
    for (sect_name, sect) in conf.items() {
        if sect.iter().any(|(k, _)| k == key) {
            conf.set(&sect_name, key, val)?;
        }
    }
    conf.write(&path)
}

/// The BrainGlobe directory, where atlases are kept (`get_brainglobe_dir`).
pub fn get_brainglobe_dir() -> Result<PathBuf> {
    let conf = read_config(None)?;
    let dir = conf
        .get("default_dirs", "brainglobe_dir")
        .ok_or_else(|| crate::Error::key("brainglobe_dir"))?;
    Ok(PathBuf::from(dir))
}

/// `brainglobe config`: check that a `*dir` value is a path whose parent
/// exists, write it, and print the configuration (`cli_modify_config`).
/// Returns what Python echoes, line by line.
pub fn cli_modify_config(key: Option<&str>, value: Option<&str>, show: bool) -> Result<String> {
    let mut out = String::new();
    if !show {
        // click passes None for options not given; Python then fails on
        // `None[-3:]` (a TypeError)
        let key =
            key.ok_or_else(|| crate::Error::Type("'NoneType' object is not subscriptable".into()))?;
        let value = value.unwrap_or("None");
        if key.ends_with("dir") {
            let path = PathBuf::from(value);
            let parent = py_parent(&path);
            let exists = parent.exists();
            out.push_str(if exists { "True\n" } else { "False\n" });
            if !exists {
                out.push_str(&format!(
                    "{value} is not a valid path. Path must be a valid path string, and its parent must exist!\n"
                ));
                return Ok(out);
            }
        }
        write_config_value(key, value, None)?;
    }
    out.push_str(&print_config()?);
    out.push('\n');
    Ok(out)
}

/// `PurePath.parent`: the parent of `a` is `.`, of `/` is `/`.
fn py_parent(p: &Path) -> PathBuf {
    match p.parent() {
        Some(q) if q.as_os_str().is_empty() => PathBuf::from("."),
        Some(q) => q.to_path_buf(),
        None => p.to_path_buf(),
    }
}

/// `_print_config`: every section (DEFAULT included) and its values.
pub fn print_config() -> Result<String> {
    let config = read_config(None)?;
    let mut s = String::new();
    for (sect_name, sect) in config.items() {
        s.push_str(&format!("[{sect_name}]\n"));
        for (k, v) in sect {
            s.push_str(&format!("\t{k}: {v}\n"));
        }
    }
    Ok(s)
}
