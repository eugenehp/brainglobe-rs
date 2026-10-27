# Changelog

## Unreleased

- `Atlas::from_store`: an atlas in an ome-zarr `Store` (a bucket read
  lazily through `LazyStore`), meshes included (`MeshReader`); every read
  fetches only what it touches. Checked against the directory reader.
- `dask` (`Atlas::new_with_dask`, `AtlasOptions::dask`), as Python's: the
  coordinate queries read one chunk of the lazy stacks.
- Builds for wasm with `zarr` (the stacks' Send/Sync bounds native-only).

## 0.1.0

First release: a Rust port of the reading half of
[brainglobe-atlasapi](https://github.com/brainglobe/brainglobe-atlasapi)
(at commit `e0f50cf`), verified against it output for output.

- The BrainGlobe Atlas API v3 format: manifests, terminologies, OME-Zarr
  templates, annotations, 4-D structure masks and hemispheres (through
  ome-zarr-rs), Draco meshes (through draco-core).
- `Atlas` and `BrainGlobeAtlas`: coordinate queries, ancestors, descendants,
  hierarchy levels, stacks, meshes, masks, additional references; downloads
  from the BrainGlobe bucket into Python's cache layout; version checks.
- The structure tree with treelib's ordering and drawing; CPython `set`
  order where the API returns `list(set(...))`.
- The `brainglobe` command (`list`, `install`, `update`, `config`) with
  click-identical parsing, help and errors, and rich-identical tables and
  panels.
- Builds for `wasm32-unknown-unknown` without the `remote` feature.
