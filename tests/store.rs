//! An atlas read from a store, and with `dask`, answers as the one read from
//! its directory does — the same metadata, structures, stacks, meshes, masks
//! and coordinate queries — and a lazily fetched store asks only for what a
//! read touches (opening: the documents; a voxel: its chunk; a mesh: its file).
#![cfg(all(feature = "zarr", not(target_arch = "wasm32")))]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use brainglobe_atlasapi::core::Atlas;
use ome_zarr::lazy::{LazyStore, Progress, settle};
use ome_zarr::store::Store;

const MANIFEST: &str = "atlases/example_mouse_100um/3_0/manifest.json";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bg/brainglobe-atlasapi")
}

fn on_disk(dask: bool) -> Atlas {
    Atlas::new_with_dask(root().join(MANIFEST), dask).unwrap()
}

fn in_store(dask: bool) -> Atlas {
    Atlas::from_store(Store::local(root(), true).unwrap(), MANIFEST, dask).unwrap()
}

/// Coordinates across the atlas and just outside it: voxels and microns.
fn coords(a: &Atlas) -> Vec<([f64; 3], bool)> {
    let shape = a.shape();
    let res = a.resolution();
    let mut out = vec![];
    for i in 0..5 {
        let f = i as f64 / 4.0;
        let v = [
            shape[0] as f64 * f * 0.99,
            shape[1] as f64 * 0.5,
            shape[2] as f64 * (1.0 - f) * 0.99,
        ];
        out.push((v, false));
        out.push(([v[0] * res[0], v[1] * res[1], v[2] * res[2]], true));
    }
    out.push(([shape[0] as f64 + 3.0, 0.0, 0.0], false));
    out
}

fn same_queries(a: &Atlas, b: &Atlas) {
    for (c, microns) in coords(a) {
        let s = |x: &Atlas| format!("{:?}", x.structure_from_coords(&c, microns, None));
        assert_eq!(s(a), s(b), "{c:?}");
        let s = |x: &Atlas| {
            format!(
                "{:?}",
                x.structure_acronym_from_coords(&c, microns, Some(2), None)
            )
        };
        assert_eq!(s(a), s(b), "{c:?}");
        let h = |x: &Atlas| format!("{:?}", x.hemisphere_from_coords(&c, microns));
        assert_eq!(h(a), h(b), "{c:?}");
    }
}

#[test]
fn a_store_reads_what_the_directory_holds() {
    let (d, s) = (on_disk(false), in_store(false));
    assert_eq!(d.metadata, s.metadata);
    assert_eq!(d.lookup_df(), s.lookup_df());
    assert_eq!(d.hierarchy().to_string(), s.hierarchy().to_string());
    assert_eq!(
        (d.template_pyramid_level(), d.annotation_pyramid_level()),
        (s.template_pyramid_level(), s.annotation_pyramid_level())
    );
    assert_eq!(
        d.template().unwrap().to_f64_vec(),
        s.template().unwrap().to_f64_vec()
    );
    assert_eq!(
        d.annotation().unwrap().to_f64_vec(),
        s.annotation().unwrap().to_f64_vec()
    );
    assert_eq!(
        d.hemispheres().unwrap().to_f64_vec(),
        s.hemispheres().unwrap().to_f64_vec()
    );
    for (acr, id, _) in d.lookup_df() {
        assert_eq!(
            d.get_structure_ancestors(id).unwrap(),
            s.get_structure_ancestors(id).unwrap()
        );
        let m = |x: &Atlas| {
            x.mesh_from_structure(id)
                .unwrap()
                .map(|m| (m.points.clone(), m.cells.clone()))
        };
        assert_eq!(m(&d), m(&s), "{acr}");
    }
    assert_eq!(
        d.get_structure_mask("CH").unwrap().to_f64_vec(),
        s.get_structure_mask("CH").unwrap().to_f64_vec()
    );
    same_queries(&d, &s);
}

#[test]
fn dask_answers_as_the_whole_stack_does() {
    let d = on_disk(false);
    same_queries(&d, &on_disk(true));
    same_queries(&d, &in_store(true));
}

#[test]
fn a_lazy_store_fetches_only_what_is_read() {
    let lazy = Arc::new(LazyStore::new());
    let fetched = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = fetched.clone();
    let fetch = move |key: String| {
        let log = log.clone();
        async move {
            log.lock().unwrap().push(key.clone());
            match std::fs::read(root().join(&key)) {
                Ok(b) => Ok(Some(b)),
                Err(_) => Ok(None),
            }
        }
    };
    let mut progress = Progress::default();
    let run = |step: &mut dyn FnMut() -> brainglobe_atlasapi::Result<()>,
               progress: &mut Progress| {
        futures::executor::block_on(settle(
            &lazy,
            || step().map_err(|e| ome_zarr::error::Error::Storage(e.to_string())),
            &fetch,
            8,
            &mut |_| {},
            progress,
        ))
        .unwrap();
    };
    let chunks = |log: &[String]| {
        log.iter()
            .filter(|k| k.contains("/c/"))
            .cloned()
            .collect::<Vec<_>>()
    };

    // opening: the manifest, the terminology and the stacks' metadata (and,
    // as Python's __init__, the masks' annotation_values)
    let store = lazy.store();
    let mut atlas = None;
    run(
        &mut || {
            atlas = Some(Atlas::from_store(store.clone(), MANIFEST, true)?);
            Ok(())
        },
        &mut progress,
    );
    let atlas = atlas.unwrap();
    let opened = chunks(&fetched.lock().unwrap());
    assert!(
        opened.iter().all(|k| k.contains("/annotation_values/")),
        "{opened:?}"
    );
    assert!(
        fetched
            .lock()
            .unwrap()
            .iter()
            .any(|k| k.ends_with("terminology.csv"))
    );

    // a voxel: one annotation chunk (and the masks it never touches stay unread)
    let n = fetched.lock().unwrap().len();
    let shape = atlas.shape();
    let c = [
        shape[0] as f64 / 2.0,
        shape[1] as f64 / 2.0,
        shape[2] as f64 / 3.0,
    ];
    let mut id = 0;
    run(
        &mut || {
            id = atlas.structure_from_coords(&c, false, None)?;
            Ok(())
        },
        &mut progress,
    );
    let new: Vec<String> = fetched.lock().unwrap()[n..].to_vec();
    assert_eq!(chunks(&new).len(), 1, "{new:?}");
    assert!(chunks(&new)[0].contains("annotations_compressed.ome.zarr"));
    assert_eq!(
        id,
        on_disk(false)
            .structure_from_coords(&c, false, None)
            .unwrap()
    );

    // a mesh: its file
    let n = fetched.lock().unwrap().len();
    let ch = atlas.structures.get("CH").unwrap().id();
    run(
        &mut || {
            atlas.mesh_from_structure(ch)?;
            Ok(())
        },
        &mut progress,
    );
    let new: Vec<String> = fetched.lock().unwrap()[n..].to_vec();
    assert_eq!(
        new,
        vec![format!(
            "annotation-sets/example_mouse-annotation/3_0/annotations.precomputed/mesh/{ch}"
        )]
    );
}
