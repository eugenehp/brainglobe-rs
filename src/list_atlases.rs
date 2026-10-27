//! Listing the available and downloaded atlases
//! (`brainglobe_atlasapi.list_atlases`).

use crate::config;
use crate::descriptors;
use crate::error::Result;
use crate::utils;

/// `3_0` -> `3.0` (`folder_version_to_dotted`).
pub fn folder_version_to_dotted(version: Option<&str>) -> Option<String> {
    version.map(|v| v.replace('_', "."))
}

fn atlases_dir() -> Result<std::path::PathBuf> {
    Ok(config::get_brainglobe_dir()?
        .join("brainglobe-atlasapi")
        .join(descriptors::V3_ATLAS_ROOTDIR))
}

/// The downloaded atlases, sorted (`get_downloaded_atlases`).
pub fn get_downloaded_atlases() -> Result<Vec<String>> {
    let dir = atlases_dir()?;
    let mut out = vec![];
    if !dir.exists() {
        return Ok(out);
    }
    for e in std::fs::read_dir(&dir)?.flatten() {
        if e.path().is_dir() {
            out.push(e.file_name().to_string_lossy().to_string());
        }
    }
    out.sort();
    Ok(out)
}

/// The latest version of a downloaded atlas, dotted
/// (`get_local_atlas_version`); `None` (with Python's message) if there is
/// none.
pub fn get_local_atlas_version(atlas_name: &str) -> Result<Option<String>> {
    let atlas_dir = atlases_dir()?.join(atlas_name);
    if let Some(p) = atlas_dir.parent() {
        std::fs::create_dir_all(p)?;
    }
    let versions = std::fs::read_dir(&atlas_dir).map(|rd| {
        rd.flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect::<Vec<_>>()
    });
    let latest = versions
        .ok()
        .and_then(|mut v| utils::get_latest_version(&mut v).ok());
    match latest {
        Some(v) => Ok(folder_version_to_dotted(Some(&v))),
        None => {
            println!("No atlas found with the name: {atlas_name}");
            Ok(None)
        }
    }
}

/// Every atlas and its latest version, from the bucket's
/// `last_versions.conf` (cached; the cache when offline), plus the user's
/// `custom_atlases.conf` (`get_all_atlases_lastversions`).
#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
pub fn get_all_atlases_lastversions() -> Result<Vec<(String, String)>> {
    let v2_dir = descriptors::V3_ATLAS_ROOTDIR;
    let base = config::get_brainglobe_dir()?
        .join("brainglobe-atlasapi")
        .join(v2_dir);
    let cache_path = base.join("last_versions.conf");
    let custom_path = base.join("custom_atlases.conf");
    let official = if utils::check_internet_connection(None, 5, false)? {
        utils::conf_from_url(
            &descriptors::remote_url_s3_http(&format!("{v2_dir}/last_versions.conf")),
            &cache_path,
        )?
    } else {
        println!("Cannot fetch latest atlas versions from the server.");
        utils::conf_from_file(&cache_path)?
    };
    let mut out = official
        .section_items("atlases")
        .ok_or_else(|| crate::Error::key("atlases"))?;
    if let Ok(custom) = utils::conf_from_file(&custom_path)
        && let Some(items) = custom.section_items("atlases")
    {
        for (k, v) in items {
            match out.iter_mut().find(|(kk, _)| *kk == k) {
                Some(e) => e.1 = v,
                None => out.push((k, v)),
            }
        }
    }
    Ok(out)
}

/// A downloaded atlas, for the listing (`get_atlases_lastversions` values).
#[derive(Debug, Clone, PartialEq)]
pub struct AtlasInfo {
    pub downloaded: bool,
    pub local: String,
    pub version: Option<String>,
    pub latest_version: String,
    pub updated: Option<bool>,
}

/// The downloaded atlases that the bucket lists, with their versions
/// (`get_atlases_lastversions`).
#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
pub fn get_atlases_lastversions() -> Result<Vec<(String, AtlasInfo)>> {
    let available = get_all_atlases_lastversions()?;
    lastversions_of(&available)
}

#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
fn lastversions_of(available: &[(String, String)]) -> Result<Vec<(String, AtlasInfo)>> {
    let mut out = vec![];
    for name in get_downloaded_atlases()? {
        if let Some((_, latest)) = available.iter().find(|(k, _)| *k == name) {
            let local_version = get_local_atlas_version(&name)?;
            out.push((
                name.clone(),
                AtlasInfo {
                    downloaded: true,
                    local: name.clone(),
                    updated: Some(local_version.as_deref() == Some(latest.as_str())),
                    version: local_version,
                    latest_version: latest.clone(),
                },
            ));
        }
    }
    Ok(out)
}

/// Every atlas: the downloaded ones (sorted), then the others (sorted).
#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
pub fn atlas_table_rows() -> Result<Vec<(String, AtlasInfo)>> {
    let available = get_all_atlases_lastversions()?;
    let downloaded = lastversions_of(&available)?;
    Ok(table_rows(&available, &downloaded))
}

/// The rows of `show_atlases`' table: the downloaded atlases (sorted), then
/// the available ones not downloaded (sorted).
pub fn table_rows(
    available: &[(String, String)],
    downloaded: &[(String, AtlasInfo)],
) -> Vec<(String, AtlasInfo)> {
    let mut rows = downloaded.to_vec();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let mut others: Vec<(String, AtlasInfo)> = available
        .iter()
        .filter(|(k, _)| !downloaded.iter().any(|(d, _)| d == k))
        .map(|(k, v)| {
            (
                k.clone(),
                AtlasInfo {
                    downloaded: false,
                    local: String::new(),
                    version: Some(String::new()),
                    latest_version: v.clone(),
                    updated: None,
                },
            )
        })
        .collect();
    others.sort_by(|a, b| a.0.cmp(&b.0));
    rows.extend(others);
    rows
}

/// Print the table of local and available atlases (`show_atlases`).
#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
pub fn show_atlases(show_local_path: bool, table_width: usize) -> Result<()> {
    let rows = atlas_table_rows()?;
    print!(
        "{}",
        crate::rich::atlases_table(&rows, show_local_path, table_width)
    );
    Ok(())
}
