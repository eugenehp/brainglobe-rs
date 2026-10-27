//! Structures (brain regions) and their dual indexing by id or acronym
//! (`brainglobe_atlasapi.structure_class`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::mesh::Mesh;
use crate::structure_tree_util::{Tree, get_structures_tree};

/// One row of a terminology CSV, as `load_structures_from_csv` returns it.
///
/// `name` / `acronym` are `None` where pandas reads a missing value (NaN):
/// an empty cell, or one of `NaN`, `NULL`, `nan`, `N/A`, `na`, `null`.
#[derive(Debug, Clone, PartialEq)]
pub struct StructureRecord {
    pub id: i64,
    pub parent_structure_id: Option<i64>,
    pub name: Option<String>,
    pub acronym: Option<String>,
    pub rgb_triplet: Option<[u8; 3]>,
    pub structure_id_path: Option<Vec<u32>>,
    /// The CSV's columns, in file order (their original names).
    pub columns: Vec<String>,
    /// Every other column (e.g. `annotation_value`), typed as pandas infers.
    pub extra: Map<String, Value>,
}

impl StructureRecord {
    /// A record with the five fields a structure needs
    /// (`descriptors.STRUCTURE_TEMPLATE`).
    pub fn new(
        id: i64,
        acronym: &str,
        name: &str,
        structure_id_path: Vec<u32>,
        rgb: [u8; 3],
    ) -> Self {
        StructureRecord {
            id,
            parent_structure_id: (structure_id_path.len() > 1)
                .then(|| structure_id_path[structure_id_path.len() - 2] as i64),
            name: Some(name.to_string()),
            acronym: Some(acronym.to_string()),
            rgb_triplet: Some(rgb),
            structure_id_path: Some(structure_id_path),
            columns: vec![],
            extra: Map::new(),
        }
    }

    pub fn path(&self) -> &[u32] {
        self.structure_id_path.as_deref().unwrap_or(&[])
    }

    /// The acronym as Python formats it (`nan` when missing).
    pub fn acronym_str(&self) -> String {
        self.acronym.clone().unwrap_or_else(|| "nan".into())
    }

    /// The record as the dict Python builds, in its key order (the CSV's
    /// columns, renamed), with JSON values (`None` for a missing parent).
    pub fn to_json(&self) -> Map<String, Value> {
        let mut m = Map::new();
        let opt_str = |s: &Option<String>| s.clone().map(Value::String).unwrap_or(Value::Null);
        fn rename(c: &str) -> &str {
            match c {
                "identifier" => "id",
                "parent_identifier" => "parent_structure_id",
                "abbreviation" => "acronym",
                "root_identifier_path" => "structure_id_path",
                "color_hex_triplet" => "rgb_triplet",
                other => other,
            }
        }
        let cols: Vec<String> = if self.columns.is_empty() {
            ["acronym", "id", "name", "structure_id_path", "rgb_triplet"]
                .iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            self.columns.iter().map(|c| rename(c).to_string()).collect()
        };
        for c in cols {
            let v = match c.as_str() {
                "id" => Value::from(self.id),
                "parent_structure_id" => self
                    .parent_structure_id
                    .map(Value::from)
                    .unwrap_or(Value::Null),
                "name" => opt_str(&self.name),
                "acronym" => opt_str(&self.acronym),
                "rgb_triplet" => self
                    .rgb_triplet
                    .map(|t| Value::from(t.to_vec()))
                    .unwrap_or(Value::Null),
                "structure_id_path" => self
                    .structure_id_path
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
                other => self.extra.get(other).cloned().unwrap_or(Value::Null),
            };
            m.insert(c, v);
        }
        m
    }
}

/// How a mesh file's bytes are read when it is not a file on disk (an atlas
/// in a store: [`crate::core::AtlasRoot::Store`]): `Ok(None)` when there is
/// no such file.
#[derive(Clone)]
pub struct MeshReader(pub Arc<ReadMesh>);

/// What a [`MeshReader`] calls: a mesh file's path to its bytes (shared
/// across threads natively; wasm's stores are single-threaded).
#[cfg(not(target_arch = "wasm32"))]
pub type ReadMesh = dyn Fn(&Path) -> Result<Option<Vec<u8>>> + Send + Sync;
#[cfg(target_arch = "wasm32")]
pub type ReadMesh = dyn Fn(&Path) -> Result<Option<Vec<u8>>>;

impl std::fmt::Debug for MeshReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MeshReader")
    }
}

/// A structure: its record, its mesh file, and the mesh, read the first time
/// it is asked for (`Structure`).
#[derive(Debug)]
pub struct Structure {
    pub record: StructureRecord,
    pub mesh_filename: Option<PathBuf>,
    mesh: OnceLock<Mesh>,
    reader: Option<MeshReader>,
}

impl Clone for Structure {
    fn clone(&self) -> Self {
        let mesh = OnceLock::new();
        if let Some(m) = self.mesh.get() {
            let _ = mesh.set(m.clone());
        }
        Structure {
            record: self.record.clone(),
            mesh_filename: self.mesh_filename.clone(),
            mesh,
            reader: self.reader.clone(),
        }
    }
}

impl Structure {
    pub fn new(record: StructureRecord, mesh_filename: Option<PathBuf>) -> Structure {
        Structure {
            record,
            mesh_filename,
            mesh: OnceLock::new(),
            reader: None,
        }
    }

    pub fn id(&self) -> i64 {
        self.record.id
    }
    pub fn acronym(&self) -> String {
        self.record.acronym_str()
    }
    pub fn name(&self) -> Option<&str> {
        self.record.name.as_deref()
    }
    pub fn structure_id_path(&self) -> &[u32] {
        self.record.path()
    }
    pub fn rgb_triplet(&self) -> Option<[u8; 3]> {
        self.record.rgb_triplet
    }

    /// `structure["mesh"]`: read (downloading it first if it is not on disk,
    /// with the `remote` feature) and cache the mesh.
    ///
    /// No mesh file: `Ok(None)`, with the warning Python gives.
    pub fn mesh(&self) -> Result<Option<&Mesh>> {
        if let Some(m) = self.mesh.get() {
            return Ok(Some(m));
        }
        let Some(file_name) = &self.mesh_filename else {
            log::warn!("No mesh filename for region {}", self.acronym());
            return Ok(None);
        };
        let read = || -> Result<Mesh> {
            if let Some(reader) = &self.reader {
                let bytes = (reader.0)(file_name)?.ok_or_else(|| {
                    Error::file_not_found(format!("Mesh file {} not found.", file_name.display()))
                })?;
                return Mesh::from_draco(&bytes);
            }
            if !file_name.exists() {
                download_mesh(file_name)?;
            }
            Mesh::read_precomputed(file_name)
        };
        let mesh = read().map_err(|e| {
            Error::Runtime(format!(
                "Failed to read mesh for region {} from file {}: {e}",
                self.acronym(),
                file_name.display()
            ))
        })?;
        let _ = self.mesh.set(mesh);
        Ok(self.mesh.get())
    }

    /// Whether the mesh has been read.
    pub fn mesh_loaded(&self) -> bool {
        self.mesh.get().is_some()
    }
}

/// Download a mesh from the bucket if it is not cached
/// (`Structure._download_mesh`): the last six components of the local path
/// are its key under `atlas/`.
#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
pub fn download_mesh(file_name: &std::path::Path) -> Result<()> {
    let parts: Vec<String> = file_name
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let root_path = parts[parts.len().saturating_sub(6)..].join("/");
    let fs = crate::remote::S3FileSystem::anon();
    if !fs.exists(&root_path)? {
        return Err(Error::file_not_found(format!(
            "Mesh file {} not found locally or remotely.",
            file_name.display()
        )));
    }
    if let Err(e) = fs.get_file(&root_path, file_name) {
        let _ = std::fs::remove_file(file_name);
        return Err(e);
    }
    Ok(())
}

#[cfg(not(all(feature = "remote", not(target_arch = "wasm32"))))]
pub fn download_mesh(file_name: &std::path::Path) -> Result<()> {
    Err(Error::file_not_found(format!(
        "Mesh file {} not found locally or remotely.",
        file_name.display()
    )))
}

/// Structures indexed by id, also reachable by acronym (`StructuresDict`).
#[derive(Debug, Clone)]
pub struct StructuresDict {
    /// id -> structure, in the order of the list
    data: Vec<Structure>,
    by_id: BTreeMap<i64, usize>,
    acronym_to_id_map: Vec<(String, i64)>,
    pub tree: Tree,
}

/// A structure key: an id, or an acronym (`structures[5]`,
/// `structures["CH"]`, or `structures["5"]`, which Python reads as an id).
#[derive(Debug, Clone, PartialEq)]
pub enum StructureKey {
    Id(i64),
    Acronym(String),
}

impl From<i64> for StructureKey {
    fn from(v: i64) -> Self {
        StructureKey::Id(v)
    }
}
impl From<i32> for StructureKey {
    fn from(v: i32) -> Self {
        StructureKey::Id(v as i64)
    }
}
impl From<u32> for StructureKey {
    fn from(v: u32) -> Self {
        StructureKey::Id(v as i64)
    }
}
impl From<&str> for StructureKey {
    fn from(v: &str) -> Self {
        StructureKey::Acronym(v.to_string())
    }
}
impl From<String> for StructureKey {
    fn from(v: String) -> Self {
        StructureKey::Acronym(v)
    }
}
impl From<&String> for StructureKey {
    fn from(v: &String) -> Self {
        StructureKey::Acronym(v.clone())
    }
}

impl StructuresDict {
    /// Build from records, each with its mesh file (`StructuresDict(list)`).
    pub fn new(structures: Vec<(StructureRecord, Option<PathBuf>)>) -> Result<StructuresDict> {
        let records: Vec<StructureRecord> = structures.iter().map(|(r, _)| r.clone()).collect();
        let tree = get_structures_tree(&records)?;
        let mut d = StructuresDict {
            data: vec![],
            by_id: BTreeMap::new(),
            acronym_to_id_map: vec![],
            tree,
        };
        for (r, mesh) in structures {
            let acr = r.acronym_str();
            match d.acronym_to_id_map.iter_mut().find(|(a, _)| *a == acr) {
                Some(e) => e.1 = r.id,
                None => d.acronym_to_id_map.push((acr, r.id)),
            }
            let st = Structure::new(r, mesh);
            match d.by_id.get(&st.id()) {
                // a repeated id replaces the earlier one, in its place
                Some(&i) => d.data[i] = st,
                None => {
                    d.by_id.insert(st.id(), d.data.len());
                    d.data.push(st);
                }
            }
        }
        Ok(d)
    }

    /// Read every structure's mesh through `reader` instead of from disk
    /// (an atlas in a store).
    pub fn set_mesh_reader(&mut self, reader: MeshReader) {
        for s in &mut self.data {
            s.reader = Some(reader.clone());
        }
    }

    /// Resolve a key to an id: `int(item)`, else the acronym's id.
    pub fn resolve(&self, item: impl Into<StructureKey>) -> Result<i64> {
        match item.into() {
            StructureKey::Id(i) => Ok(i),
            StructureKey::Acronym(a) => match crate::utils::parse_py_int(&a) {
                Some(i) => Ok(i),
                None => self
                    .acronym_to_id_map
                    .iter()
                    .find(|(k, _)| *k == a)
                    .map(|(_, v)| *v)
                    .ok_or_else(|| Error::key(&a)),
            },
        }
    }

    /// `structures[item]`.
    pub fn get(&self, item: impl Into<StructureKey>) -> Result<&Structure> {
        let id = self.resolve(item)?;
        self.by_id
            .get(&id)
            .map(|&i| &self.data[i])
            .ok_or_else(|| Error::key_int(id))
    }

    pub fn contains(&self, item: impl Into<StructureKey>) -> bool {
        self.get(item).is_ok()
    }

    /// The ids, in insertion order (`structures.keys()`).
    pub fn keys(&self) -> Vec<i64> {
        self.data.iter().map(Structure::id).collect()
    }

    /// `structures.values()`.
    pub fn values(&self) -> impl Iterator<Item = &Structure> {
        self.data.iter()
    }

    /// `structures.items()`.
    pub fn items(&self) -> impl Iterator<Item = (i64, &Structure)> {
        self.data.iter().map(|s| (s.id(), s))
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// acronym -> id (`acronym_to_id_map`).
    pub fn acronym_to_id_map(&self) -> &[(String, i64)] {
        &self.acronym_to_id_map
    }

    /// The acronyms of every ancestor of a structure, root first
    /// (`Atlas.get_structure_ancestors`).
    pub fn ancestors(&self, structure: impl Into<StructureKey>) -> Result<Vec<String>> {
        let path = self.get(structure)?.structure_id_path();
        let n = path.len().saturating_sub(1);
        path[..n]
            .iter()
            .map(|id| Ok(self.get(*id as i64)?.acronym()))
            .collect()
    }

    /// The acronyms of every descendant of a structure, in structure order
    /// (`Atlas.get_structure_descendants`).
    pub fn descendants(&self, structure: impl Into<StructureKey>) -> Result<Vec<String>> {
        let acronym = self.get(structure)?.acronym();
        let mut out = vec![];
        for s in self.values() {
            if self.ancestors(s.id())?.contains(&acronym) {
                out.push(s.acronym());
            }
        }
        Ok(out)
    }

    /// The structures at a hierarchy level within the subtree of `structure`
    /// (`Atlas.get_structures_at_hierarchy_level`), as ids.
    ///
    /// With a level: for every leaf under the structure, the id at that level
    /// of its path, deduplicated in the order Python's `list(set(...))`
    /// gives. Without: the structure's ancestors, then its descendants
    /// breadth first.
    pub fn at_hierarchy_level(
        &self,
        structure: impl Into<StructureKey>,
        hierarchy_level: Option<i64>,
    ) -> Result<Vec<i64>> {
        let key = structure.into();
        if let Some(l) = hierarchy_level
            && l < 0
        {
            return Err(Error::value("hierarchy_level must be non-negative"));
        }
        let input = self.get(key.clone()).map_err(|_| {
            let shown = match &key {
                StructureKey::Id(i) => i.to_string(),
                StructureKey::Acronym(a) => a.clone(),
            };
            Error::Key(crate::utils::py_repr_str(&format!(
                "Structure '{shown}' not found in atlas"
            )))
        })?;
        let input_id = input.id();
        match hierarchy_level {
            None => {
                let mut result: Vec<i64> = input
                    .structure_id_path()
                    .iter()
                    .map(|&i| i as i64)
                    .collect();
                let mut seen: std::collections::HashSet<i64> = result.iter().copied().collect();
                let mut queue = std::collections::VecDeque::from([input_id]);
                while let Some(cur) = queue.pop_front() {
                    for child in self.tree.children(cur)? {
                        if seen.insert(child.identifier) {
                            result.push(child.identifier);
                            queue.push_back(child.identifier);
                        }
                    }
                }
                Ok(result)
            }
            Some(level) => {
                let mut leaves: Vec<i64> = self
                    .tree
                    .leaves(Some(input_id))?
                    .iter()
                    .map(|n| n.identifier)
                    .collect();
                if leaves.is_empty() {
                    leaves.push(input_id);
                }
                let mut ids = vec![];
                for leaf in leaves {
                    let path = self.get(leaf)?.structure_id_path();
                    match path.get(level as usize) {
                        Some(&v) => ids.push(v as i64),
                        None => {
                            return Err(Error::value(format!(
                                "Structure {} has no descendants at hierarchy level {level}",
                                input.acronym()
                            )));
                        }
                    }
                }
                Ok(crate::pyset::py_set_list(ids))
            }
        }
    }

    /// [`at_hierarchy_level`](StructuresDict::at_hierarchy_level) with
    /// `as_acronym=True`.
    pub fn at_hierarchy_level_acronyms(
        &self,
        structure: impl Into<StructureKey>,
        hierarchy_level: Option<i64>,
    ) -> Result<Vec<String>> {
        self.at_hierarchy_level(structure, hierarchy_level)?
            .into_iter()
            .map(|id| Ok(self.get(id)?.acronym()))
            .collect()
    }
}

impl std::fmt::Display for StructuresDict {
    /// `repr(structures)`: the tree.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.tree.show())
    }
}
