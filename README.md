# brainglobe-atlasapi (Rust)

A Rust implementation of
[brainglobe-atlasapi](https://github.com/brainglobe/brainglobe-atlasapi):
read [BrainGlobe](https://brainglobe.info) atlases — rat, mouse (adult and
developing), zebrafish, macaque, human, cat, cuttlefish, fly and the rest of
the ~230 BrainGlobe publishes — in the BrainGlobe Atlas API v3 format, with
the same results as the Python package.

* **The v3 format**: atlases as components on BrainGlobe's bucket — a
  manifest, a terminology (the structure tree, CSV), a template and an
  annotation set (OME-Zarr 0.5 multiscales: labels, 4-D per-structure masks,
  hemispheres) and Draco meshes — downloaded on demand into the same cache
  layout Python uses, so either can read the other's downloads.
* **Output for output parity**: structures, trees, coordinate queries,
  stacks, meshes and masks are identical to Python's (array bytes and mesh
  coordinates compared by SHA-256); so are the `brainglobe` command's
  output, help and errors, and the rich tables and panels it prints.
* **Reuses** [ome-zarr-rs](https://github.com/eugenehp/ome-zarr-rs) for the
  OME-Zarr stacks and [draco-core](https://crates.io/crates/draco-core) (pure
  Rust) for the meshes, which it decodes to the same points and faces as
  DracoPy.
* **WebAssembly**: without the `remote` feature the library builds for
  `wasm32-unknown-unknown`; without `zarr` too (`--no-default-features`) it is
  terminologies, structure trees and Draco meshes from bytes, no C codecs (the
  atlas viewer's `brainglobe` module: ~150 kB gzipped).

## Command line

```bash
cargo install --path .

brainglobe list                       # every atlas, local and available
brainglobe list -s                    # ... with local paths
brainglobe install -a whs_sd_rat_39um # download one
brainglobe update -a whs_sd_rat_39um  # to the latest version (-f: re-download)
brainglobe config -s                  # show the configuration
brainglobe config -k brainglobe_dir -v /data/brainglobe
```

The options, help page, usage errors and exit codes are click's, character
for character (`-h` wraps to the terminal as click does).

## Library

```rust
use brainglobe_atlasapi::bg_atlas::{AtlasOptions, BrainGlobeAtlas};

let atlas = BrainGlobeAtlas::new("whs_sd_rat_39um", AtlasOptions::default())?;

// coordinates (voxels, or microns with `true`) -> structure and hemisphere
let id = atlas.structure_from_coords(&[512.0, 200.0, 250.0], false, None)?;
let acronym = atlas.structure_acronym_from_coords(&[512.0, 200.0, 250.0], false, None, None)?;
let side = atlas.hemisphere_name_from_coords(&[512.0, 200.0, 250.0], false)?;

// the hierarchy
println!("{}", atlas.hierarchy());                      // the tree, as treelib draws it
let ancestors = atlas.get_structure_ancestors("CPu")?;  // acronyms, root first
let at_3 = atlas.get_structures_at_hierarchy_level("Brain", Some(3))?;

// stacks (cached; `*_lazy` for lazy arrays, Python's `dask=True`)
let annotation = atlas.annotation()?;                   // uint32 labels
let template = atlas.template()?;
let mask = atlas.get_structure_mask("Cb")?;             // uint8, structure and descendants

// meshes, in microns, (z, y, x)
let mesh = atlas.mesh_from_structure("Cb")?.unwrap();
println!("{} points, {} triangles", mesh.points.len(), mesh.cells.len());
```

An atlas already on disk opens with `Atlas::new(<root>/atlases/<name>/<version>/manifest.json)`.

| Python | Rust |
|---|---|
| `bg_atlas.BrainGlobeAtlas` | [`bg_atlas`](src/bg_atlas.rs) (feature `remote`) |
| `core.Atlas`, `AdditionalRefDict` | [`core`](src/core.rs) (feature `zarr`) |
| `structure_class.Structure`, `StructuresDict` | [`structure_class`](src/structure_class.rs) |
| `structure_tree_util` (+ treelib's `Tree`) | [`structure_tree_util`](src/structure_tree_util.rs) |
| `Structure._read_mesh` (DracoPy, meshio) | [`mesh`](src/mesh.rs) |
| `list_atlases`, `update_atlases` | [`list_atlases`](src/list_atlases.rs), [`update_atlases`](src/update_atlases.rs) |
| `config` (+ `configparser`) | [`config`](src/config.rs), [`configparser`](src/configparser.rs) |
| `descriptors`, `atlas_name` | [`descriptors`](src/descriptors.rs), [`atlas_name`](src/atlas_name.rs) |
| `utils` (requests, s3fs) | [`utils`](src/utils.rs), [`remote`](src/remote.rs) |
| `cli` (click) | [`cli`](src/cli.rs) |
| the rich output | [`rich`](src/rich.rs), [`emoji_codes`](src/emoji_codes.rs) |
| `list(set(ids))` order | [`pyset`](src/pyset.rs) |

An atlas need not be a directory: `Atlas::from_store` opens the same v3
layout in an ome-zarr `Store` — a bucket read through ome-zarr's `LazyStore`,
say, which asks the host for each file a read touches (opening: the manifest,
the terminology and the stacks' metadata; a voxel: its chunk; a mesh: its
file). With `dask` (`Atlas::new_with_dask`, `AtlasOptions::dask`, as Python's
`dask=True`) coordinate queries read the lazy stacks one chunk at a time
rather than the whole stack. Together they let a browser use the Python API
on an atlas it never downloads whole:

```rust
use std::sync::Arc;
use brainglobe_atlasapi::core::Atlas;
use ome_zarr::lazy::{LazyStore, settle};

let lazy = Arc::new(LazyStore::new());
let atlas = settle(&lazy, || Atlas::from_store(lazy.store(), "atlases/whs_sd_rat_39um/3_0/manifest.json", true)
    .map_err(|e| ome_zarr::error::Error::Storage(e.to_string())), &fetch, 16, &mut |_| {}, &mut progress).await?;
// then any call the same way: atlas.structure_from_coords(...), atlas.mesh_from_structure("Cx"),
// atlas.annotation_lazy()?.read(&[512..513, 0..512, 0..512]) (Python's atlas.annotation[512, :, :])
```

## Tests

```bash
cargo test                      # parity with the Python fixtures
cargo test -- --ignored         # also download an atlas and compare the cache
```

`tests/fixtures` holds reference outputs written by brainglobe-atlasapi
(commit `e0f50cf`, with the environment of `scripts/requirements-parity.txt`)
by `scripts/gen_parity_fixtures.py`:

* 26 real terminologies (mouse, rat, human, macaque, fish, fly, cuttlefish,
  bird, ...): the parsed structures, the tree drawing, node order, leaves and
  traversals, the acronym map, and for sampled structures their ancestors,
  descendants and the structures at every hierarchy level (whose order is
  CPython's `set` iteration order, reproduced in `pyset`);
* the complete `example_mouse_100um` atlas (2.5 MB) and everything Python
  reads from it: metadata, lookup table, hierarchy, the three stacks, 310
  coordinate queries (voxels, microns, hierarchy levels, out-of-range errors),
  every mesh and every 4-D mask;
* the atlas panel for every manifest on the bucket at five widths (with
  rich's pretty-printing and its full emoji table), the `list` table, and
  wrapped messages;
* the configuration files (Python's `configparser` output), and the CLI.

`tests/remote.rs` downloads the example atlas from scratch and checks that
the files on disk are those Python left, byte for byte. `tests/store.rs` opens the example atlas
from a store and with `dask`, and checks every answer against the directory
reader's — and that a lazily fetched store asks only for what each read
touches. Errors carry the
Python exception type (`Error::py_type`) and its message.

### WebAssembly

```bash
scripts/check-wasm.sh           # cargo build --target wasm32-unknown-unknown --no-default-features
```

ome-zarr's zstd and blosc codecs are C and need an LLVM clang
(`brew install llvm`). Without `remote` there is no network or `BrainGlobeAtlas`;
the terminology parser (`load_structures_from_csv_bytes`), the structure tree
and queries, the Draco mesh decoder and the rich rendering all work on bytes
the host fetches. With `--features zarr` (an LLVM clang for its C codecs)
the stacks too: `Atlas::from_store` over a `LazyStore` the host fills.

## Differences from brainglobe-atlasapi

* Errors are values. A command that fails prints the exception's last
  traceback line (`ValueError: ...`) and exits 1, as Python does, without the
  traceback above it.
* Downloads use the bucket's public HTTPS endpoint (anonymous `ListObjectsV2`
  and `GET`), 16 at a time; s3fs is not involved. `BRAINGLOBE_S3_ENDPOINT`
  points at a mirror. Progress bars (tqdm) are not drawn.
* Python's keyword arguments are option structs (`AtlasOptions`); `as_acronym`
  / `as_string` variants are separate methods
  (`structure_acronym_from_coords`, `hemisphere_name_from_coords`,
  `get_structures_at_hierarchy_level_acronyms`).
* The configuration paths are read when used, not when the module is imported.
* Not ported (yet): `atlas_generation`, the packaging pipeline that makes
  atlases (meshing, writing, validation). This crate reads them.

## License

BSD-3-Clause, as brainglobe-atlasapi.
