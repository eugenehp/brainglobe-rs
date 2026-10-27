//! # brainglobe-atlasapi
//!
//! A Rust implementation of
//! [brainglobe-atlasapi](https://github.com/brainglobe/brainglobe-atlasapi):
//! reading [BrainGlobe](https://brainglobe.info) atlases — rat, mouse
//! (adult and developing), zebrafish, macaque, human and more — in the
//! BrainGlobe Atlas API v3 format: their OME-Zarr stacks, structure trees,
//! Draco meshes and coordinate queries, downloaded from BrainGlobe's bucket
//! on demand.
//!
//! | module | Python counterpart |
//! |---|---|
//! | [`bg_atlas`] | `brainglobe_atlasapi.bg_atlas` — [`BrainGlobeAtlas`] (feature `remote`) |
//! | [`core`] | `brainglobe_atlasapi.core` — [`Atlas`] (from a directory, or lazily from a store), `AdditionalRefDict` |
//! | [`structure_class`] | `brainglobe_atlasapi.structure_class` — `Structure`, `StructuresDict` |
//! | [`structure_tree_util`] | `brainglobe_atlasapi.structure_tree_util` (and the treelib `Tree` it builds) |
//! | [`mesh`] | `Structure._read_mesh` (DracoPy + meshio) |
//! | [`list_atlases`], [`update_atlases`] | `brainglobe_atlasapi.list_atlases`, `.update_atlases` |
//! | [`config`], [`configparser`] | `brainglobe_atlasapi.config` (and Python's `configparser`) |
//! | [`descriptors`], [`atlas_name`] | `brainglobe_atlasapi.descriptors`, `.atlas_name` |
//! | [`utils`], [`remote`] | `brainglobe_atlasapi.utils` (web requests: s3fs / requests) |
//! | [`cli`] | `brainglobe_atlasapi.cli` (click) |
//! | [`rich`] | the rich tables and panels it prints |
//!
//! ```no_run
//! use brainglobe_atlasapi::bg_atlas::{AtlasOptions, BrainGlobeAtlas};
//!
//! let atlas = BrainGlobeAtlas::new("allen_mouse_25um", AtlasOptions::default())?;
//! let id = atlas.structure_from_coords(&[264.0, 160.0, 228.0], false, None)?;
//! println!("{} {:?}", atlas.structures.get(id)?.acronym(), atlas.get_structure_ancestors(id)?);
//! let mesh = atlas.mesh_from_structure("CH")?.unwrap();
//! println!("{} points", mesh.points.len());
//! # Ok::<(), brainglobe_atlasapi::Error>(())
//! ```

// Index loops mirror the reference (CPython / difflib / rich) implementations.
#![allow(clippy::needless_range_loop)]

pub mod atlas_name;
#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
pub mod bg_atlas;
pub mod cli;
pub mod config;
pub mod configparser;
#[cfg(feature = "zarr")]
pub mod core;
pub mod descriptors;
pub mod emoji_codes;
pub mod error;
pub mod list_atlases;
pub mod mesh;
pub mod pyset;
#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
pub mod remote;
pub mod rich;
pub mod structure_class;
pub mod structure_tree_util;
#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
pub mod update_atlases;
pub mod utils;

pub use error::{Error, Result};

#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
pub use bg_atlas::BrainGlobeAtlas;
#[cfg(feature = "zarr")]
pub use core::Atlas;
