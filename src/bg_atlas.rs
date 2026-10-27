//! `BrainGlobeAtlas`: an [`Atlas`] fetched from the BrainGlobe bucket by name,
//! with version checks (`brainglobe_atlasapi.bg_atlas`). Feature `remote`.

use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

use crate::config;
use crate::core::Atlas;
use crate::descriptors::*;
use crate::error::{Error, Result};
use crate::list_atlases::get_all_atlases_lastversions;
use crate::remote::{S3FileSystem, check_internet_connection, check_s3_status};
use crate::utils::{get_latest_version, read_json};

/// `_version_tuple_from_str`: `"3.0"` -> `[3, 0]`.
pub fn version_tuple_from_str(version_str: &str) -> Result<Vec<i64>> {
    version_str
        .split('.')
        .map(|n| {
            crate::utils::parse_py_int(n).ok_or_else(|| {
                Error::value(format!(
                    "invalid literal for int() with base 10: {}",
                    crate::utils::py_repr_str(n)
                ))
            })
        })
        .collect()
}

/// `_version_str_from_tuple`: `[3, 0]` -> `"3_0"`.
pub fn version_str_from_tuple(version: &[i64]) -> String {
    version
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join("_")
}

/// Options of [`BrainGlobeAtlas::new`] (Python's keyword arguments).
#[derive(Default)]
pub struct AtlasOptions {
    /// Desired version (`"3.0"` or `"3_0"`); the latest when `None`.
    pub version: Option<String>,
    /// Folder for BrainGlobe downloads; the config's when `None`.
    pub brainglobe_dir: Option<PathBuf>,
    /// Skip the remote version check when `false` (default `true`).
    pub check_latest: Option<bool>,
    /// The config file to read (Python's `config_dir`, which is a file path).
    pub config_dir: Option<PathBuf>,
    /// Called during downloads with (completed, total) bytes.
    pub fn_update: Option<Box<dyn FnMut(u64, u64) + Send>>,
    /// `dask`: coordinate queries read the lazy stacks (see [`Atlas::new_with_dask`]).
    pub dask: bool,
}

/// An atlas fetched by name (`BrainGlobeAtlas`). It dereferences to the
/// [`Atlas`] it opened.
pub struct BrainGlobeAtlas {
    pub atlas_name: String,
    /// `<brainglobe_dir>/brainglobe-atlasapi`
    pub brainglobe_dir: PathBuf,
    requested_version: Option<String>,
    local_version: Option<Vec<i64>>,
    remote_version: Option<Vec<i64>>,
    local_full_name: Option<String>,
    fs: S3FileSystem,
    atlas: Option<Atlas>,
}

impl Deref for BrainGlobeAtlas {
    type Target = Atlas;
    fn deref(&self) -> &Atlas {
        self.atlas.as_ref().expect("atlas opened")
    }
}

impl DerefMut for BrainGlobeAtlas {
    fn deref_mut(&mut self) -> &mut Atlas {
        self.atlas.as_mut().expect("atlas opened")
    }
}

impl BrainGlobeAtlas {
    /// Open `atlas_name`, downloading it first if it is not on disk.
    pub fn new(atlas_name: &str, opts: AtlasOptions) -> Result<BrainGlobeAtlas> {
        let requested_version = opts.version.as_ref().map(|v| v.replace('.', "_"));
        let local_version = match &opts.version {
            Some(v) => Some(version_tuple_from_str(&v.replace('_', "."))?),
            None => None,
        };
        let conf = config::read_config(opts.config_dir.as_deref())?;
        let brainglobe_dir = match opts.brainglobe_dir {
            Some(d) => d,
            None => PathBuf::from(
                conf.get("default_dirs", "brainglobe_dir")
                    .ok_or_else(|| Error::key("brainglobe_dir"))?,
            ),
        }
        .join("brainglobe-atlasapi");
        std::fs::create_dir_all(&brainglobe_dir)?;

        let mut a = BrainGlobeAtlas {
            atlas_name: atlas_name.to_string(),
            brainglobe_dir,
            requested_version,
            local_version,
            remote_version: None,
            local_full_name: None,
            fs: S3FileSystem::anon(),
            atlas: None,
        };

        if a.local_full_name()?.is_none() {
            if a.remote_version()?.is_none() {
                check_internet_connection(None, 5, true)?;
                // the internet is up, so the atlas name was invalid
                return Err(Error::value(format!(
                    "{atlas_name} is not a valid atlas name!"
                )));
            }
            a.download()?;
            if a.local_full_name()?.is_none() {
                return Err(Error::Assertion(
                    "Download failed: local atlas manifest not found after download.".into(),
                ));
            }
        }
        let name = a.local_full_name()?.unwrap();
        let manifest = a.brainglobe_dir.join(name);
        a.atlas = Some(Atlas::new_with_dask(manifest, opts.dask)?);

        if opts.check_latest.unwrap_or(true) {
            a.check_latest_version(true)?;
        }
        Ok(a)
    }

    /// The manifest of the requested version if it is on disk, else of the
    /// latest version on disk, relative to `brainglobe_dir`
    /// (`local_full_name`).
    pub fn local_full_name(&mut self) -> Result<Option<String>> {
        if self.local_full_name.is_some() {
            return Ok(self.local_full_name.clone());
        }
        let atlases = self.brainglobe_dir.join(V3_ATLAS_ROOTDIR);
        std::fs::create_dir_all(&atlases)?;
        let dir = atlases.join(&self.atlas_name);
        let mut available: Vec<String> = vec![];
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            entries.sort();
            for p in entries {
                if !p.join("manifest.json").exists() {
                    continue;
                }
                let v = p.file_name().unwrap().to_string_lossy().to_string();
                let ok = match &self.requested_version {
                    Some(r) => &v == r,
                    None => is_version_dir(&v),
                };
                if ok {
                    available.push(v);
                }
            }
        }
        if available.is_empty() {
            return Ok(None);
        }
        let latest = get_latest_version(&mut available)?;
        self.local_full_name = Some(format!(
            "{V3_ATLAS_ROOTDIR}/{}/{latest}/manifest.json",
            self.atlas_name
        ));
        Ok(self.local_full_name.clone())
    }

    /// The version of the files on disk (`local_version`).
    pub fn local_version(&mut self) -> Result<Vec<i64>> {
        if let Some(v) = &self.local_version {
            return Ok(v.clone());
        }
        let v = self
            .atlas
            .as_ref()
            .and_then(|a| a.metadata.get("version"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::key("version"))?
            .replace('_', ".");
        let t = version_tuple_from_str(&v)?;
        self.local_version = Some(t.clone());
        Ok(t)
    }

    /// The version to download (`remote_version`): the requested one if the
    /// bucket has it, else the one `last_versions.conf` lists. `None`
    /// offline, or for an atlas only on disk.
    pub fn remote_version(&mut self) -> Result<Option<Vec<i64>>> {
        if self.remote_version.is_some() {
            return Ok(self.remote_version.clone());
        }
        if !check_s3_status(5, false)? {
            return Ok(None);
        }
        let bucket_path = remote_url_s3(&format!("atlases/{}", self.atlas_name));
        if !self.fs.exists(&bucket_path)? {
            if self.local_full_name()?.is_some() {
                return Ok(None);
            }
            return Err(Error::file_not_found(format!(
                "{} is not a valid atlas name!",
                self.atlas_name
            )));
        }
        match self.requested_version.clone() {
            None => {
                let last_versions = get_all_atlases_lastversions()?;
                let Some(v) = last_versions.iter().find(|(k, _)| *k == self.atlas_name) else {
                    if self.local_full_name()?.is_some() {
                        return Ok(None);
                    }
                    return Err(Error::file_not_found(format!(
                        "{} has no released version available. Specify a version explicitly to download it.",
                        self.atlas_name
                    )));
                };
                self.remote_version = Some(version_tuple_from_str(&v.1.replace('_', "."))?);
            }
            Some(req) => {
                let requested_path = format!("{bucket_path}/{req}");
                if !self.fs.exists(&requested_path)? {
                    return Err(Error::file_not_found(format!(
                        "Requested version {req} for atlas {} not found in remote.",
                        self.atlas_name
                    )));
                }
                self.remote_version = Some(version_tuple_from_str(&req.replace('_', "."))?);
            }
        }
        Ok(self.remote_version.clone())
    }

    /// Download the atlas's manifest and the metadata of its components; the
    /// stacks' chunks and the meshes come later, when first read
    /// (`download`). On failure the manifest is removed, so the next run
    /// retries.
    pub fn download(&mut self) -> Result<()> {
        check_s3_status(5, true)?;
        let remote = self
            .remote_version()?
            .ok_or_else(|| Error::Type("'NoneType' object is not iterable".into()))?;
        let remote_version_str = version_str_from_tuple(&remote);
        let key_name = format!(
            "{V3_ATLAS_ROOTDIR}/{}/{remote_version_str}/manifest.json",
            self.atlas_name
        );
        let local_path = self.brainglobe_dir.join(&key_name);
        if let Some(p) = local_path.parent() {
            std::fs::create_dir_all(p)?;
        }
        println!(
            "Downloading {} atlas v{} manifest:",
            self.atlas_name,
            remote_version_str.replace('_', ".")
        );
        self.fs.get(&remote_url_s3(&key_name), &local_path, false)?;
        let r = self.download_components(&local_path, &remote_version_str);
        if r.is_err() {
            let _ = std::fs::remove_file(&local_path);
        }
        r
    }

    fn download_components(&mut self, manifest: &Path, remote_version_str: &str) -> Result<()> {
        let metadata = read_json(manifest)?;
        let loc = |keys: &[&str]| -> Result<String> {
            let mut v = metadata.get(keys[0]).ok_or_else(|| Error::key(keys[0]))?;
            for k in &keys[1..] {
                v = v.get(*k).ok_or_else(|| Error::key(k))?;
            }
            Ok(v.get("location")
                .and_then(|l| l.as_str())
                .ok_or_else(|| Error::key("location"))?
                .chars()
                .skip(1)
                .collect())
        };
        let name = |key: &str| -> String {
            metadata
                .get(key)
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let bg = self.brainglobe_dir.clone();

        // terminology
        let terminology_location = loc(&["terminology"])?;
        let local_terminology = bg.join(&terminology_location);
        if !local_terminology.exists() {
            println!(
                "Downloading terminology metadata for {}:",
                name("terminology")
            );
            self.fs.get(
                &remote_url_s3(&terminology_location),
                &local_terminology,
                true,
            )?;
        }

        // coordinate space
        let coordspace_location = loc(&["coordinate_space"])?;
        let local_coordspace = bg.join(&coordspace_location);
        if !local_coordspace.exists() {
            println!(
                "Downloading coordinate space metadata for {}:",
                name("coordinate_space")
            );
            self.fs.get(
                &remote_url_s3(&coordspace_location),
                &local_coordspace,
                true,
            )?;
        }

        // annotation metadata
        let annotation_location = loc(&["annotation_set"])?;
        let local_annotation = bg.join(&annotation_location);
        if !local_annotation.exists() {
            println!(
                "Downloading annotation metadata for {}:",
                name("annotation_set")
            );
            self.fs.get(
                &remote_url_s3(&format!(
                    "{annotation_location}/{V3_ANNOTATION_NAME}/**/*.json"
                )),
                &local_annotation.join(V3_ANNOTATION_NAME),
                false,
            )?;
            std::fs::create_dir_all(local_annotation.join(V3_MESHES_DIRECTORY))?;

            // 4-D masks metadata (JSON only; chunks are read lazily)
            let masks = (|| -> Result<()> {
                self.fs.get(
                    &remote_url_s3(&format!(
                        "{annotation_location}/{V3_ANNOTATION_MASKS_NAME}/**/*.json"
                    )),
                    &local_annotation.join(V3_ANNOTATION_MASKS_NAME),
                    false,
                )?;
                self.fs.get(
                    &remote_url_s3(&format!(
                        "{annotation_location}/{V3_ANNOTATION_MASKS_NAME}/{V3_ANNOTATION_MAP_NAME}"
                    )),
                    &local_annotation.join(V3_ANNOTATION_MASKS_NAME),
                    true,
                )
            })();
            if let Err(Error::FileNotFound(_)) = masks {
                return Err(Error::file_not_found(format!(
                    "Annotation masks metadata not found for atlas {} v{}.",
                    self.atlas_name,
                    remote_version_str.replace('_', ".")
                )));
            }
            masks?;

            if !metadata
                .get("symmetric")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                self.fs.get(
                    &remote_url_s3(&format!(
                        "{annotation_location}/{V3_HEMISPHERES_NAME}/**/*.json"
                    )),
                    &local_annotation.join(V3_HEMISPHERES_NAME),
                    false,
                )?;
            }
        }

        // template metadata
        let template_location = loc(&["annotation_set", "template"])?;
        let local_template = bg.join(&template_location);
        if !local_template.exists() {
            println!("Downloading template metadata for {}:", name("template"));
            self.fs.get(
                &remote_url_s3(&format!("{template_location}/{V3_TEMPLATE_NAME}/**/*.json")),
                &local_template.join(V3_TEMPLATE_NAME),
                false,
            )?;
        }

        // additional references
        if let Some(refs) = metadata
            .get("additional_references")
            .and_then(|v| v.as_array())
        {
            for r in refs {
                let ref_location: String = r
                    .get("location")
                    .and_then(|l| l.as_str())
                    .ok_or_else(|| Error::key("location"))?
                    .chars()
                    .skip(1)
                    .collect();
                let local = bg.join(&ref_location);
                if !local.exists() {
                    // (Python names the main template here)
                    println!("Downloading template metadata for {}:", name("template"));
                    self.fs.get(
                        &remote_url_s3(&format!("{ref_location}/{V3_TEMPLATE_NAME}/**/*.json")),
                        &local.join(V3_TEMPLATE_NAME),
                        false,
                    )?;
                }
            }
        }
        // look the manifest up again
        self.local_full_name = None;
        Ok(())
    }

    /// Whether the local version is the latest (`check_latest_version`):
    /// `Some(false)` (with a message, if `print_warning`) when it is not,
    /// `None` offline.
    pub fn check_latest_version(&mut self, print_warning: bool) -> Result<Option<bool>> {
        let Some(remote) = self.remote_version()? else {
            return Ok(None);
        };
        let local = version_str_from_tuple(&self.local_version()?);
        let online = version_str_from_tuple(&remote);
        if local != online {
            if print_warning {
                crate::rich::rprint(&format!(
                    "[b][magenta2]brainglobe_atlasapi[/b]: [b]{}[/b] version [b]{}[/b] is not the latest available ([b]{}[/b]). To update the atlas run in the terminal:[/magenta2]\n [gold1]brainglobe update -a {}[/gold1]",
                    self.atlas_name,
                    local.replace('_', "."),
                    online.replace('_', "."),
                    self.atlas_name
                ));
            }
            return Ok(Some(false));
        }
        Ok(Some(true))
    }

    /// The version requested, as a folder name (`3_0`).
    pub fn requested_version(&self) -> Option<&str> {
        self.requested_version.as_deref()
    }

    /// `repr(atlas)`: e.g. `allen mouse atlas (res. 25um)`.
    pub fn repr(&self) -> String {
        let mut parts: Vec<&str> = self.atlas_name.split('_').collect();
        let res = parts.pop().unwrap_or("");
        format!("{} atlas (res. {res})", parts.join(" "))
    }

    /// `str(atlas)`: the metadata panel, as rich prints it without colours.
    pub fn to_rich_string(&self) -> Result<String> {
        crate::rich::atlas_metadata_panel(&self.atlas_name, &self.metadata)
    }
}

/// `\d+(?:_\d+)?`: a version folder name.
fn is_version_dir(v: &str) -> bool {
    let mut parts = v.splitn(2, '_');
    let a = parts.next().unwrap_or("");
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    ok(a) && parts.next().is_none_or(ok)
}
