//! Downloads from the BrainGlobe bucket (network: `cargo test -- --ignored`).
//!
//! The example atlas is fetched from scratch into a temporary BrainGlobe
//! directory and read as `scripts/gen_parity_fixtures.py` reads it; the
//! files on disk must then be those Python left in `tests/fixtures/bg`, byte
//! for byte, so either implementation can use the other's cache.

use std::collections::BTreeMap;
use std::path::Path;

use brainglobe_atlasapi::bg_atlas::{AtlasOptions, BrainGlobeAtlas};

fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().to_string();
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[test]
#[ignore = "network"]
fn download_matches_python_layout() {
    let tmp = tempfile::tempdir().unwrap();
    let bg = tmp.path().join("bg");
    let cfg = tmp.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("bg_config.conf"),
        format!(
            "[default_dirs]\nbrainglobe_dir = {0}\ninterm_download_dir = {0}\n\n",
            bg.display()
        ),
    )
    .unwrap();
    // SAFETY: one test in this binary.
    unsafe { std::env::set_var("BRAINGLOBE_CONFIG_DIR", &cfg) };

    let atlas = BrainGlobeAtlas::new(
        "example_mouse_100um",
        AtlasOptions {
            check_latest: Some(false),
            brainglobe_dir: Some(bg.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    atlas.template().unwrap();
    atlas.annotation().unwrap();
    atlas.hemispheres().unwrap();
    for id in atlas.structures.keys() {
        atlas.mesh_from_structure(id).unwrap().unwrap();
        atlas.get_structure_mask(id).unwrap();
    }

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bg");
    let mut want = files(&fixture);
    let mut got = files(&bg);
    // the bucket's version list changes over time; compare its presence only
    let conf = "brainglobe-atlasapi/atlases/last_versions.conf";
    assert_eq!(want.remove(conf).is_some(), got.remove(conf).is_some());
    let want_names: Vec<&String> = want.keys().collect();
    let got_names: Vec<&String> = got.keys().collect();
    assert_eq!(got_names, want_names, "the same files");
    for (k, v) in &want {
        assert!(got[k] == *v, "{k} differs");
    }
}
