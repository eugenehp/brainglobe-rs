//! The core `Atlas` class (`brainglobe_atlasapi.core`): an atlas on disk in
//! the v3 layout, its stacks, structures, meshes and coordinate queries.
//!
//! An atlas lives under a root directory as its components:
//!
//! ```text
//! <root>/atlases/<name>/<version>/manifest.json      metadata
//! <root>/terminologies/<t>/<version>/terminology.csv the structure tree
//! <root>/templates/<t>/<version>/template.ome.zarr   reference image
//! <root>/annotation-sets/<a>/<version>/
//!     annotations_compressed.ome.zarr                labels (uint32)
//!     annotations.ome.zarr                           4-D masks, one per structure
//!     hemispheres.ome.zarr                           1 left, 2 right (asymmetric atlases)
//!     annotations.precomputed/mesh/<id>              Draco meshes
//! ```
//!
//! Python's `dask=True` (arrays returned lazily) is the `*_lazy` methods
//! here; the plain ones read and cache the whole stack as `.compute()` does.
//! An atlas opened with `dask` set ([`Atlas::new_with_dask`]) also answers
//! coordinate queries from the lazy stacks, reading one voxel's chunk.
//!
//! The components are read from a directory ([`Atlas::new`], Python's
//! `Atlas(path)`), or from an ome-zarr [`Store`] holding the same layout
//! ([`Atlas::from_store`]): a bucket read through a
//! [`LazyStore`](ome_zarr::lazy::LazyStore), say, where opening the atlas
//! fetches its manifest, terminology and stack metadata, and every read only
//! the chunks and meshes it touches — what Python's `dask=True` does over
//! fsspec, for a host (a browser) that fetches on its own.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ome_zarr::array::{LazyArray, NdArray};
use ome_zarr::classes::OmeZarrMultiscale;
use ome_zarr::store::Store;
use ome_zarr::writer::GroupLike;
use ome_zarr::zarr::{Group, Mode};
use serde_json::{Map, Value};

use crate::descriptors::*;
use crate::error::{Error, Result};
use crate::mesh::Mesh;
use crate::structure_class::{MeshReader, StructureKey, StructureRecord, StructuresDict};
use crate::structure_tree_util::Tree;
use crate::utils::{
    load_structures_from_csv, load_structures_from_csv_bytes, py_repr_str, read_json,
};

/// `np.isclose(a, b)`: `|a - b| <= atol + rtol * |b|`.
fn isclose(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-8 + 1e-5 * b.abs()
}

pub use crate::utils::py_float_repr;

/// Python's `repr` of a JSON number as it was parsed (`100` stays `100`).
fn py_num_repr(v: &Value) -> String {
    match v {
        Value::Number(n) if n.is_f64() => py_float_repr(n.as_f64().unwrap()),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// `tuple(values)` as Python prints it.
pub fn py_tuple_repr(values: &[String]) -> String {
    if values.len() == 1 {
        format!("({},)", values[0])
    } else {
        format!("({})", values.join(", "))
    }
}

/// The pyramid level whose spatial scale is `resolution` (microns, z y x)
/// (`_determine_pyramid_level`).
pub fn determine_pyramid_level(
    multiscale: &OmeZarrMultiscale,
    resolution: &[f64],
) -> Result<usize> {
    for (idx, image) in multiscale.images().iter().enumerate() {
        let dims = &image.axes[image.axes.len().saturating_sub(3)..];
        let scales: Vec<f64> = dims
            .iter()
            .map(|d| image.scale.get(d).unwrap_or(1.0))
            .collect();
        if resolution
            .iter()
            .zip(&scales)
            .all(|(res, scale)| isclose(res / 1000.0, *scale))
        {
            return Ok(idx);
        }
    }
    Err(Error::value(format!(
        "Requested resolution {} um is invalid.",
        py_tuple_repr(
            &resolution
                .iter()
                .map(|r| py_float_repr(*r))
                .collect::<Vec<_>>()
        )
    )))
}

fn open_multiscale(path: &Path) -> Result<OmeZarrMultiscale> {
    Ok(OmeZarrMultiscale::from_ome_zarr(
        path.to_string_lossy().as_ref(),
    )?)
}

/// `metadata[k]["location"][1:]`.
fn location(metadata: &Map<String, Value>, keys: &[&str]) -> Result<String> {
    let mut v: &Value = metadata.get(keys[0]).ok_or_else(|| Error::key(keys[0]))?;
    for k in &keys[1..] {
        v = v.get(*k).ok_or_else(|| Error::key(k))?;
    }
    let loc = v
        .get("location")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::key("location"))?;
    Ok(loc.chars().skip(1).collect())
}

/// Download one pyramid level of a stack when its chunks are not on disk
/// (`if not (resolution_path / "c").exists(): fs.get(...)`).
fn ensure_level(
    root_dir: &Path,
    component_location: &str,
    zarr_name: &str,
    dataset_path: &str,
    what: &str,
) -> Result<()> {
    let resolution_path = root_dir
        .join(component_location)
        .join(zarr_name)
        .join(dataset_path);
    if resolution_path.join("c").exists() {
        return Ok(());
    }
    #[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
    {
        println!("Downloading {what}...");
        let fs = crate::remote::S3FileSystem::anon();
        fs.get(
            &remote_url_s3(&format!("{component_location}/{zarr_name}/{dataset_path}/")),
            &resolution_path,
            true,
        )?;
        Ok(())
    }
    #[cfg(not(all(feature = "remote", not(target_arch = "wasm32"))))]
    {
        let _ = what;
        Err(Error::file_not_found(format!(
            "{} has no chunks on disk (build with the `remote` feature to download them)",
            resolution_path.display()
        )))
    }
}

/// Where an atlas's components are read from.
#[derive(Clone)]
pub enum AtlasRoot {
    /// a directory in the v3 layout (`root_dir`)
    Dir(PathBuf),
    /// an ome-zarr store holding the layout, its components under `prefix`
    /// (`""`, or ending in `/`)
    Store { store: Store, prefix: String },
}

impl std::fmt::Debug for AtlasRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AtlasRoot::Dir(p) => write!(f, "Dir({})", p.display()),
            AtlasRoot::Store { prefix, .. } => write!(f, "Store({prefix:?})"),
        }
    }
}

impl AtlasRoot {
    fn key(prefix: &str, rel: &str) -> String {
        format!("{prefix}{}", rel.trim_start_matches('/'))
    }

    /// A component's path: on disk, or its key in the store.
    pub fn path(&self, rel: &str) -> PathBuf {
        match self {
            AtlasRoot::Dir(d) => d.join(rel),
            AtlasRoot::Store { prefix, .. } => PathBuf::from(Self::key(prefix, rel)),
        }
    }

    /// A file's bytes (`FileNotFoundError` when there is none).
    pub fn read(&self, rel: &str) -> Result<Vec<u8>> {
        match self {
            AtlasRoot::Dir(d) => Ok(std::fs::read(d.join(rel))?),
            AtlasRoot::Store { store, prefix } => {
                let key = Self::key(prefix, rel);
                Ok(store
                    .get(&key)?
                    .ok_or_else(|| Error::file_not_found(format!("No such file: {key}")))?
                    .to_vec())
            }
        }
    }

    /// Is there a Zarr group or array at `rel`?
    fn has_zarr(&self, rel: &str) -> bool {
        match self {
            AtlasRoot::Dir(d) => d.join(rel).exists(),
            AtlasRoot::Store { store, prefix } => {
                let key = Self::key(prefix, rel);
                ["zarr.json", ".zgroup", ".zattrs"]
                    .iter()
                    .any(|m| matches!(store.get(&format!("{key}/{m}")), Ok(Some(_))))
            }
        }
    }

    fn group(&self, rel: &str) -> Result<Group> {
        match self {
            AtlasRoot::Dir(d) => Ok(Group::open_url(
                &d.join(rel).to_string_lossy(),
                Mode::Read,
                None,
            )?),
            AtlasRoot::Store { store, prefix } => Ok(Group::open(
                store,
                &Self::key(prefix, rel),
                Mode::Read,
                None,
            )?),
        }
    }

    fn multiscale(&self, rel: &str) -> Result<OmeZarrMultiscale> {
        match self {
            AtlasRoot::Dir(d) => open_multiscale(&d.join(rel)),
            AtlasRoot::Store { .. } => Ok(OmeZarrMultiscale::from_ome_zarr(GroupLike::Group(
                self.group(rel)?,
            ))?),
        }
    }

    /// A stack's level on hand: downloaded into a directory if its chunks
    /// are not there yet; a store reads them as they are asked for.
    fn ensure_level(
        &self,
        component_location: &str,
        zarr_name: &str,
        dataset_path: &str,
        what: &str,
    ) -> Result<()> {
        match self {
            AtlasRoot::Dir(d) => ensure_level(d, component_location, zarr_name, dataset_path, what),
            AtlasRoot::Store { .. } => Ok(()),
        }
    }
}

/// The hemisphere value of the left hemisphere.
pub const LEFT_HEMISPHERE_VALUE: u8 = 1;
/// The hemisphere value of the right hemisphere.
pub const RIGHT_HEMISPHERE_VALUE: u8 = 2;

/// An atlas on disk (`brainglobe_atlasapi.core.Atlas`).
pub struct Atlas {
    /// the manifest (`metadata`); editable, as in Python (e.g. `shape`)
    pub metadata: Map<String, Value>,
    /// the root of the v3 layout (`root_dir`: the manifest's `parents[3]`;
    /// for an atlas in a store, its key prefix)
    pub root_dir: PathBuf,
    /// where the components are read from
    pub root: AtlasRoot,
    /// `dask`: coordinate queries read the lazy stacks (one chunk), not the
    /// whole stack
    pub dask: bool,
    /// the terminology's rows (`structures_list`)
    pub structures_list: Vec<StructureRecord>,
    pub structures: StructuresDict,
    /// additional reference images (`additional_references`)
    pub additional_references: AdditionalRefDict,
    template_pyramid_level: usize,
    annotation_pyramid_level: usize,
    annotation_masks_pyramid_level: Option<usize>,
    annotation_mapping: Option<HashMap<i64, usize>>,
    template: OnceLock<NdArray>,
    annotation: OnceLock<NdArray>,
    hemispheres: OnceLock<NdArray>,
}

impl std::fmt::Debug for Atlas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Atlas")
            .field("root_dir", &self.root_dir)
            .field("name", &self.metadata.get("name"))
            .finish()
    }
}

impl Atlas {
    /// Open the atlas whose `manifest.json` is at `path` (`Atlas(path)`).
    pub fn new(path: impl AsRef<Path>) -> Result<Atlas> {
        Self::new_with_dask(path, false)
    }

    /// `Atlas(path, dask=dask)`.
    pub fn new_with_dask(path: impl AsRef<Path>, dask: bool) -> Result<Atlas> {
        let atlas_path = path.as_ref().to_path_buf();
        let root_dir = atlas_path
            .ancestors()
            .nth(4)
            .ok_or_else(|| Error::Index("4".into()))?
            .to_path_buf();
        let metadata = read_json(&atlas_path)?;
        Self::open(AtlasRoot::Dir(root_dir.clone()), root_dir, metadata, dask)
    }

    /// An atlas in a store holding the v3 layout: `manifest_key` is its
    /// manifest's key (`atlases/<name>/<version>/manifest.json` under the
    /// components' root: for BrainGlobe's bucket, the store at `.../atlas/`).
    /// Read lazily (a [`LazyStore`](ome_zarr::lazy::LazyStore)), opening it
    /// asks for the manifest, the terminology and the stacks' metadata.
    pub fn from_store(store: Store, manifest_key: &str, dask: bool) -> Result<Atlas> {
        let parts: Vec<&str> = manifest_key.trim_start_matches('/').split('/').collect();
        if parts.len() < 4 {
            return Err(Error::Index("4".into()));
        }
        let prefix: String = parts[..parts.len() - 4]
            .iter()
            .map(|p| format!("{p}/"))
            .collect();
        let root = AtlasRoot::Store {
            store,
            prefix: prefix.clone(),
        };
        let bytes = root.read(&parts[parts.len() - 4..].join("/"))?;
        let metadata: Map<String, Value> = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Value(format!("{manifest_key}: {e}")))?;
        Self::open(root, PathBuf::from(prefix), metadata, dask)
    }

    fn open(
        root: AtlasRoot,
        root_dir: PathBuf,
        metadata: Map<String, Value>,
        dask: bool,
    ) -> Result<Atlas> {
        let structures_rel = format!(
            "{}/{V3_TERMINOLOGY_NAME}",
            location(&metadata, &["terminology"])?
        );
        let structures_list = match &root {
            AtlasRoot::Dir(d) => load_structures_from_csv(d.join(&structures_rel))?,
            AtlasRoot::Store { .. } => {
                load_structures_from_csv_bytes(&root.read(&structures_rel)?)?
            }
        };
        let annotation_location = location(&metadata, &["annotation_set"])?;
        let meshes_path = root.path(&format!("{annotation_location}/{V3_MESHES_DIRECTORY}"));

        let resolution = Self::resolution_of(&metadata)?;
        let template_location = location(&metadata, &["annotation_set", "template"])?;
        let template_pyramid_level = determine_pyramid_level(
            &root.multiscale(&format!("{template_location}/{V3_TEMPLATE_NAME}"))?,
            &resolution,
        )?;
        let annotation_rel = format!("{annotation_location}/{V3_ANNOTATION_NAME}");
        let annotation_pyramid_level =
            determine_pyramid_level(&root.multiscale(&annotation_rel)?, &resolution)?;

        let masks_rel = format!("{annotation_location}/{V3_ANNOTATION_MASKS_NAME}");
        let (annotation_masks_pyramid_level, annotation_mapping) = if root.has_zarr(&masks_rel) {
            let level = determine_pyramid_level(&root.multiscale(&masks_rel)?, &resolution)?;
            let values = root
                .group(&masks_rel)?
                .array(V3_ANNOTATION_MAP_NAME)?
                .read_all()?;
            let mapping = values
                .to_f64_vec()
                .into_iter()
                .enumerate()
                .map(|(i, v)| (v as i64, i))
                .collect();
            (Some(level), Some(mapping))
        } else {
            (None, None)
        };

        let mut structures = StructuresDict::new(
            structures_list
                .iter()
                .map(|s| (s.clone(), Some(meshes_path.join(s.id.to_string()))))
                .collect(),
        )?;
        if let AtlasRoot::Store { store, .. } = &root {
            let store = store.clone();
            structures.set_mesh_reader(MeshReader(std::sync::Arc::new(move |path: &Path| {
                Ok(store.get(&path.to_string_lossy())?.map(|b| b.to_vec()))
            })));
        }

        let additional_references = match metadata.get("additional_references") {
            Some(Value::Array(refs)) => AdditionalRefDict::new(refs, root.clone(), &resolution)?,
            _ => {
                log::warn!(
                    "This atlas seems to be outdated as no additional_references list is found in metadata!"
                );
                AdditionalRefDict::new(&[], root.clone(), &resolution)?
            }
        };

        Ok(Atlas {
            metadata,
            root_dir,
            root,
            dask,
            structures_list,
            structures,
            additional_references,
            template_pyramid_level,
            annotation_pyramid_level,
            annotation_masks_pyramid_level,
            annotation_mapping,
            template: OnceLock::new(),
            annotation: OnceLock::new(),
            hemispheres: OnceLock::new(),
        })
    }

    fn resolution_of(metadata: &Map<String, Value>) -> Result<Vec<f64>> {
        metadata
            .get("resolution")
            .and_then(Value::as_array)
            .map(|r| r.iter().filter_map(Value::as_f64).collect())
            .ok_or_else(|| Error::key("resolution"))
    }

    /// `resolution`, in microns.
    pub fn resolution(&self) -> Vec<f64> {
        Self::resolution_of(&self.metadata).unwrap_or_default()
    }

    /// `tuple(metadata["resolution"])` as Python prints it.
    pub fn resolution_repr(&self) -> String {
        let r: Vec<String> = self
            .metadata
            .get("resolution")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(py_num_repr).collect())
            .unwrap_or_default();
        py_tuple_repr(&r)
    }

    /// `orientation`: always the standard `asr`.
    pub fn orientation(&self) -> &'static str {
        ATLAS_ORIENTATION
    }

    /// `shape` (from the metadata).
    pub fn shape(&self) -> Vec<usize> {
        self.metadata
            .get("shape")
            .and_then(Value::as_array)
            .map(|s| {
                s.iter()
                    .filter_map(Value::as_u64)
                    .map(|v| v as usize)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `shape_um`: shape times resolution.
    pub fn shape_um(&self) -> Vec<f64> {
        self.shape()
            .iter()
            .zip(self.resolution())
            .map(|(s, r)| *s as f64 * r)
            .collect()
    }

    /// `hierarchy`: the structure tree.
    pub fn hierarchy(&self) -> &Tree {
        &self.structures.tree
    }

    /// `lookup_df`: (acronym, id, name) of every structure, in list order.
    pub fn lookup_df(&self) -> Vec<(String, i64, Option<String>)> {
        self.structures_list
            .iter()
            .map(|r| (r.acronym_str(), r.id, r.name.clone()))
            .collect()
    }

    /// `brainglobe-space`'s axis names for the atlas orientation: for `asr`,
    /// (`sagittal`, `vertical`, `frontal`).
    pub fn axes_order(&self) -> [&'static str; 3] {
        let name = |c: char| match c {
            'a' | 'p' => "sagittal",
            's' | 'i' => "vertical",
            _ => "frontal",
        };
        let o: Vec<char> = ATLAS_ORIENTATION.chars().collect();
        [name(o[0]), name(o[1]), name(o[2])]
    }

    fn template_location(&self) -> Result<String> {
        location(&self.metadata, &["annotation_set", "template"])
    }

    fn annotation_location(&self) -> Result<String> {
        location(&self.metadata, &["annotation_set"])
    }

    fn level_lazy(
        &self,
        component_location: &str,
        zarr_name: &str,
        level: usize,
        what: &str,
    ) -> Result<LazyArray> {
        let rel = format!("{component_location}/{zarr_name}");
        let ms = self.root.multiscale(&rel)?;
        let dataset_path = ms.base.metadata.datasets[level].path.clone();
        self.root
            .ensure_level(component_location, zarr_name, &dataset_path, what)?;
        // reopen: the level's chunks may have just arrived
        let ms = self.root.multiscale(&rel)?;
        Ok(ms.images()[level].data.clone())
    }

    /// The template image, lazily (`template` with `dask=True`).
    pub fn template_lazy(&self) -> Result<LazyArray> {
        self.level_lazy(
            &self.template_location()?,
            V3_TEMPLATE_NAME,
            self.template_pyramid_level,
            "template",
        )
    }

    /// The template image (`template`), read once and cached.
    pub fn template(&self) -> Result<&NdArray> {
        if let Some(t) = self.template.get() {
            return Ok(t);
        }
        let t = self.template_lazy()?.compute()?;
        Ok(self.template.get_or_init(|| t))
    }

    /// Deprecated alias of [`template`](Atlas::template) (`reference`).
    pub fn reference(&self) -> Result<&NdArray> {
        println!(
            "Warning: atlas.reference is a deprecated alias for atlas.template, and will be removed in future versions."
        );
        self.template()
    }

    /// The annotation, lazily (`annotation` with `dask=True`).
    pub fn annotation_lazy(&self) -> Result<LazyArray> {
        self.level_lazy(
            &self.annotation_location()?,
            V3_ANNOTATION_NAME,
            self.annotation_pyramid_level,
            "annotations",
        )
    }

    /// The annotation (`annotation`), read once and cached.
    pub fn annotation(&self) -> Result<&NdArray> {
        if let Some(a) = self.annotation.get() {
            return Ok(a);
        }
        let a = self.annotation_lazy()?.compute()?;
        Ok(self.annotation.get_or_init(|| a))
    }

    fn symmetric(&self) -> bool {
        self.metadata
            .get("symmetric")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// The generated hemispheres of a symmetric atlas: split in half along
    /// the frontal (left-right) axis, right first; an odd middle plane is
    /// left.
    fn symmetric_hemispheres(&self) -> NdArray {
        let shape = self.shape();
        let front = self
            .axes_order()
            .iter()
            .position(|a| *a == "frontal")
            .unwrap();
        // Python's round(): half to even
        let split = (shape[front] as f64 / 2.0).round_ties_even() as usize;
        let mut stack =
            ndarray::ArrayD::<u8>::from_elem(ndarray::IxDyn(&shape), RIGHT_HEMISPHERE_VALUE);
        stack
            .slice_axis_mut(ndarray::Axis(front), ndarray::Slice::from(split..))
            .fill(LEFT_HEMISPHERE_VALUE);
        NdArray::UInt8(stack)
    }

    /// The hemispheres, lazily (`hemispheres` with `dask=True`).
    pub fn hemispheres_lazy(&self) -> Result<LazyArray> {
        if self.symmetric() {
            return Ok(LazyArray::from_ndarray(self.symmetric_hemispheres()));
        }
        self.level_lazy(
            &self.annotation_location()?,
            V3_HEMISPHERES_NAME,
            self.annotation_pyramid_level,
            "hemispheres",
        )
    }

    /// The hemispheres, 1 left, 2 right (`hemispheres`): generated for a
    /// symmetric atlas, else read from the annotation set.
    pub fn hemispheres(&self) -> Result<&NdArray> {
        if let Some(h) = self.hemispheres.get() {
            return Ok(h);
        }
        let h = if self.symmetric() {
            self.symmetric_hemispheres()
        } else {
            self.hemispheres_lazy()?.compute()?
        };
        Ok(self.hemispheres.get_or_init(|| h))
    }

    /// Forget the cached stacks (as a fresh Python attribute would be).
    pub fn clear_cache(&mut self) {
        self.template = OnceLock::new();
        self.annotation = OnceLock::new();
        self.hemispheres = OnceLock::new();
    }

    /// `_idx_from_coords`: microns to voxels, then `int()` (towards zero).
    pub fn idx_from_coords(&self, coords: &[f64], microns: bool) -> Vec<i64> {
        let res = self.resolution();
        coords
            .iter()
            .enumerate()
            .map(|(i, &c)| {
                let c = if microns {
                    c / res.get(i).copied().unwrap_or(1.0)
                } else {
                    c
                };
                c.trunc() as i64
            })
            .collect()
    }

    /// numpy indexing: negative indices count from the end; out of range is
    /// an `IndexError`.
    fn np_index(idx: &[i64], shape: &[usize]) -> Result<Vec<usize>> {
        if idx.len() > shape.len() {
            return Err(Error::Index(format!(
                "too many indices for array: array is {}-dimensional, but {} were indexed",
                shape.len(),
                idx.len()
            )));
        }
        idx.iter()
            .zip(shape)
            .enumerate()
            .map(|(axis, (&i, &n))| {
                let j = if i < 0 { i + n as i64 } else { i };
                if j < 0 || j >= n as i64 {
                    Err(Error::Index(format!(
                        "index {i} is out of bounds for axis {axis} with size {n}"
                    )))
                } else {
                    Ok(j as usize)
                }
            })
            .collect()
    }

    fn voxel(&self, stack: &NdArray, coords: &[f64], microns: bool) -> Result<f64> {
        let idx = Self::np_index(&self.idx_from_coords(coords, microns), &stack.shape())?;
        Ok(stack.get_f64(&idx).unwrap_or(0.0))
    }

    /// One voxel of a lazy stack (`int(stack[idx].compute())`): its chunk
    /// read, not the stack.
    fn voxel_lazy(&self, stack: &LazyArray, coords: &[f64], microns: bool) -> Result<f64> {
        let shape: Vec<usize> = stack.shape().iter().map(|&n| n as usize).collect();
        let idx = Self::np_index(&self.idx_from_coords(coords, microns), &shape)?;
        let region: Vec<std::ops::Range<u64>> = (0..shape.len())
            .map(|k| match idx.get(k) {
                Some(&i) => i as u64..i as u64 + 1,
                None => 0..shape[k] as u64,
            })
            .collect();
        let v = stack.read(&region)?;
        Ok(v.get_f64(&vec![0; shape.len()]).unwrap_or(0.0))
    }

    /// The hemisphere at a coordinate triplet (`hemisphere_from_coords`).
    pub fn hemisphere_from_coords(&self, coords: &[f64], microns: bool) -> Result<u8> {
        if self.dask {
            return Ok(self.voxel_lazy(&self.hemispheres_lazy()?, coords, microns)? as u8);
        }
        Ok(self.voxel(self.hemispheres()?, coords, microns)? as u8)
    }

    /// `hemisphere_from_coords(..., as_string=True)`: `left` or `right`.
    /// Outside the hemispheres (0), Python's `["left", "right"][hem - 1]`
    /// fails (the uint8 subtraction wraps to 255).
    pub fn hemisphere_name_from_coords(
        &self,
        coords: &[f64],
        microns: bool,
    ) -> Result<&'static str> {
        match self.hemisphere_from_coords(coords, microns)? {
            1 => Ok("left"),
            2 => Ok("right"),
            _ => Err(Error::Index("list index out of range".into())),
        }
    }

    /// The structure id at a coordinate triplet (`structure_from_coords`);
    /// `hierarchy_lev` cuts it to that level of its path (Python list
    /// indexing: negative from the end).
    pub fn structure_from_coords(
        &self,
        coords: &[f64],
        microns: bool,
        hierarchy_lev: Option<i64>,
    ) -> Result<i64> {
        let mut rid = if self.dask {
            self.voxel_lazy(&self.annotation_lazy()?, coords, microns)?
        } else {
            self.voxel(self.annotation()?, coords, microns)?
        } as i64;
        if let Some(lev) = hierarchy_lev {
            let path = self.structures.get(rid)?.structure_id_path();
            rid = py_list_index(path, lev)? as i64;
        }
        Ok(rid)
    }

    /// `structure_from_coords(..., as_acronym=True)`: the acronym, or
    /// `key_error_string` (default `Outside atlas`) when the id is not a
    /// structure.
    pub fn structure_acronym_from_coords(
        &self,
        coords: &[f64],
        microns: bool,
        hierarchy_lev: Option<i64>,
        key_error_string: Option<&str>,
    ) -> Result<String> {
        let rid = self.structure_from_coords(coords, microns, hierarchy_lev)?;
        match self.structures.get(rid) {
            Ok(s) => Ok(s.acronym()),
            Err(_) => Ok(key_error_string.unwrap_or("Outside atlas").to_string()),
        }
    }

    /// `mesh_from_structure(structure)`.
    pub fn mesh_from_structure(&self, structure: impl Into<StructureKey>) -> Result<Option<&Mesh>> {
        self.structures.get(structure)?.mesh()
    }

    /// `mesh_from_structure([..])`.
    pub fn meshes_from_structures(
        &self,
        structures: &[StructureKey],
    ) -> Result<Vec<Option<&Mesh>>> {
        structures
            .iter()
            .map(|s| self.mesh_from_structure(s.clone()))
            .collect()
    }

    /// `meshfile_from_structure(structure)`.
    pub fn meshfile_from_structure(
        &self,
        structure: impl Into<StructureKey>,
    ) -> Result<Option<PathBuf>> {
        Ok(self.structures.get(structure)?.mesh_filename.clone())
    }

    /// `root_mesh()`.
    pub fn root_mesh(&self) -> Result<Option<&Mesh>> {
        self.mesh_from_structure("root")
    }

    /// `root_meshfile()`.
    pub fn root_meshfile(&self) -> Result<Option<PathBuf>> {
        self.meshfile_from_structure("root")
    }

    /// The acronyms of every ancestor of a structure, root first
    /// (`get_structure_ancestors`).
    pub fn get_structure_ancestors(
        &self,
        structure: impl Into<StructureKey>,
    ) -> Result<Vec<String>> {
        self.structures.ancestors(structure)
    }

    /// The acronyms of every descendant of a structure, in structure order
    /// (`get_structure_descendants`).
    pub fn get_structure_descendants(
        &self,
        structure: impl Into<StructureKey>,
    ) -> Result<Vec<String>> {
        self.structures.descendants(structure)
    }

    /// The structures at a hierarchy level within the subtree of `structure`
    /// (`get_structures_at_hierarchy_level`), as ids; see
    /// [`StructuresDict::at_hierarchy_level`].
    pub fn get_structures_at_hierarchy_level(
        &self,
        structure: impl Into<StructureKey>,
        hierarchy_level: Option<i64>,
    ) -> Result<Vec<i64>> {
        self.structures
            .at_hierarchy_level(structure, hierarchy_level)
    }

    /// [`get_structures_at_hierarchy_level`](Atlas::get_structures_at_hierarchy_level)
    /// with `as_acronym=True`.
    pub fn get_structures_at_hierarchy_level_acronyms(
        &self,
        structure: impl Into<StructureKey>,
        hierarchy_level: Option<i64>,
    ) -> Result<Vec<String>> {
        self.structures
            .at_hierarchy_level_acronyms(structure, hierarchy_level)
    }

    fn annotation_masks_rel(&self) -> Result<String> {
        Ok(format!(
            "{}/{V3_ANNOTATION_MASKS_NAME}",
            self.annotation_location()?
        ))
    }

    pub fn get_structure_mask(&self, structure: impl Into<StructureKey>) -> Result<NdArray> {
        let key = structure.into();
        let Some(mapping) = &self.annotation_mapping else {
            return Err(Error::file_not_found(
                "This atlas does not have a 4D mask array. Re-download the atlas to get the latest version.",
            ));
        };
        let structure_id = self.structures.get(key.clone())?.id();
        let shown = match &key {
            StructureKey::Id(i) => i.to_string(),
            StructureKey::Acronym(a) => a.clone(),
        };
        let index = *mapping.get(&structure_id).ok_or_else(|| {
            Error::Key(py_repr_str(&format!(
                "Structure {shown} (id={structure_id}) not found in annotation mapping."
            )))
        })?;
        let masks_rel = self.annotation_masks_rel()?;
        let level = self.annotation_masks_pyramid_level.unwrap_or(0);
        let ms = self.root.multiscale(&masks_rel)?;
        let dataset_path = ms.base.metadata.datasets[level].path.clone();
        let local_path = self
            .root
            .path(&masks_rel)
            .join(&dataset_path)
            .join("c")
            .join(index.to_string());
        // a store reads the mask's chunks as they are asked for
        if matches!(self.root, AtlasRoot::Dir(_)) && !local_path.exists() {
            #[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
            {
                let annotation_location = self.annotation_location()?;
                let remote_path = remote_url_s3(&format!(
                    "{annotation_location}/{V3_ANNOTATION_MASKS_NAME}/{dataset_path}/c/{index}/"
                ));
                let fs = crate::remote::S3FileSystem::anon();
                fs.get(&remote_path, &local_path, true).map_err(|e| match e {
                    Error::FileNotFound(_) => Error::file_not_found(format!(
                        "Mask for structure {shown} (id={structure_id}) not found at {remote_path}"
                    )),
                    other => other,
                })?;
            }
        }
        let ms = self.root.multiscale(&masks_rel)?;
        let data = &ms.images()[level].data;
        let shape = data.shape().to_vec();
        let mut region: Vec<std::ops::Range<u64>> = shape.iter().map(|&n| 0..n).collect();
        region[0] = index as u64..index as u64 + 1;
        let m = data.read(&region)?;
        Ok(m.reshape(&shape[1..].iter().map(|&n| n as usize).collect::<Vec<_>>())?)
    }

    /// The structure id -> 4-D mask index mapping, when the atlas has masks.
    pub fn annotation_mapping(&self) -> Option<&HashMap<i64, usize>> {
        self.annotation_mapping.as_ref()
    }

    /// The pyramid level read for the template.
    pub fn template_pyramid_level(&self) -> usize {
        self.template_pyramid_level
    }

    /// The pyramid level read for the annotation and hemispheres.
    pub fn annotation_pyramid_level(&self) -> usize {
        self.annotation_pyramid_level
    }
}

/// `list[i]` with Python's negative indices.
fn py_list_index(list: &[u32], i: i64) -> Result<u32> {
    let n = list.len() as i64;
    let j = if i < 0 { i + n } else { i };
    if j < 0 || j >= n {
        return Err(Error::Index("list index out of range".into()));
    }
    Ok(list[j as usize])
}

/// Additional reference images, read the first time they are asked for
/// (`AdditionalRefDict`).
#[derive(Debug, Clone)]
pub struct AdditionalRefDict {
    root: AtlasRoot,
    references_names: Vec<String>,
    references: Vec<Map<String, Value>>,
    resolution: Vec<f64>,
    data: HashMap<String, std::sync::Arc<OnceLock<NdArray>>>,
}

impl AdditionalRefDict {
    pub fn new(references_list: &[Value], root: AtlasRoot, resolution: &[f64]) -> Result<Self> {
        let mut references = vec![];
        let mut names = vec![];
        for r in references_list {
            let m = r.as_object().cloned().unwrap_or_default();
            let name = m
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::key("name"))?
                .to_string();
            names.push(name);
            references.push(m);
        }
        let data = names
            .iter()
            .map(|n| (n.clone(), std::sync::Arc::new(OnceLock::new())))
            .collect();
        Ok(AdditionalRefDict {
            root,
            references_names: names,
            references,
            resolution: resolution.to_vec(),
            data,
        })
    }

    /// The reference names (`list(refs)`).
    pub fn keys(&self) -> &[String] {
        &self.references_names
    }

    /// `refs[key]`: the image, or `None` (with Python's warning) for a name
    /// that is not a reference.
    pub fn get(&self, key: &str) -> Result<Option<&NdArray>> {
        let Some(i) = self.references_names.iter().position(|n| n == key) else {
            log::warn!(
                "No reference named {key} (available: [{}])",
                self.references_names
                    .iter()
                    .map(|n| py_repr_str(n))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            return Ok(None);
        };
        let cell = &self.data[key];
        if let Some(v) = cell.get() {
            return Ok(Some(v));
        }
        let loc: String = self.references[i]
            .get("location")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::key("location"))?
            .chars()
            .skip(1)
            .collect();
        let rel = format!("{loc}/{V3_TEMPLATE_NAME}");
        let ms = self.root.multiscale(&rel)?;
        let level = determine_pyramid_level(&ms, &self.resolution)?;
        let dataset_path = ms.base.metadata.datasets[level].path.clone();
        self.root
            .ensure_level(&loc, V3_TEMPLATE_NAME, &dataset_path, "template")?;
        let ms = self.root.multiscale(&rel)?;
        let v = ms.images()[level].data.compute()?;
        Ok(Some(cell.get_or_init(|| v)))
    }
}
