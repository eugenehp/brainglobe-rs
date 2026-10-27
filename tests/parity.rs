//! Parity with brainglobe-atlasapi: every fixture in `tests/fixtures/parity`
//! was written by Python (`scripts/gen_parity_fixtures.py`); the Rust
//! results must be the same.

use std::path::{Path, PathBuf};

use brainglobe_atlasapi::Error;
use brainglobe_atlasapi::cli::{Parsed, help_page, parse_args};
use brainglobe_atlasapi::core::Atlas;
use brainglobe_atlasapi::list_atlases::{AtlasInfo, table_rows};
use brainglobe_atlasapi::pyset::py_set_list;
use brainglobe_atlasapi::rich::{atlas_metadata_panel_at, atlases_table_at, rprint_string};
use brainglobe_atlasapi::structure_class::StructuresDict;
use brainglobe_atlasapi::structure_tree_util::*;
use brainglobe_atlasapi::utils::{
    atlas_name_from_repr, atlas_repr_from_name, get_latest_version, load_structures_from_csv,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn load(name: &str) -> Value {
    let p = fixtures().join("parity").join(name);
    serde_json::from_str(
        &std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())),
    )
    .unwrap()
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// A Rust result as the fixture records a Python call: `{"ok": v}` or
/// `{"err": {"type", "msg"}}`.
fn rec<T: Into<Value>>(r: Result<T, Error>) -> Value {
    match r {
        Ok(v) => json!({ "ok": v.into() }),
        Err(e) => json!({ "err": { "type": e.py_type(), "msg": e.to_string() } }),
    }
}

#[test]
fn pyset_order() {
    for case in load("pyset.json").as_array().unwrap() {
        let input: Vec<i64> = serde_json::from_value(case["in"].clone()).unwrap();
        let out: Vec<i64> = serde_json::from_value(case["out"].clone()).unwrap();
        assert_eq!(py_set_list(input.clone()), out, "list(set({input:?}))");
    }
}

#[test]
fn terminologies() {
    let all = load("terminology.json");
    for (name, f) in all.as_object().unwrap() {
        let csv = fixtures().join("terminology").join(format!("{name}.csv"));
        let structures = load_structures_from_csv(&csv).unwrap();
        let got: Vec<Value> = structures
            .iter()
            .map(|s| Value::Object(s.to_json()))
            .collect();
        assert_eq!(
            Value::Array(got),
            f["structures"],
            "{name}: load_structures_from_csv"
        );

        let tree = get_structures_tree(&structures).unwrap();
        assert_eq!(
            tree.show(),
            f["show"].as_str().unwrap(),
            "{name}: tree.show()"
        );
        let ids = |v: Vec<&Node>| json!(v.iter().map(|n| n.identifier).collect::<Vec<_>>());
        assert_eq!(
            json!(
                tree.nodes()
                    .iter()
                    .map(|n| n.identifier)
                    .collect::<Vec<_>>()
            ),
            f["nodes"],
            "{name}: nodes"
        );
        assert_eq!(
            json!(
                tree.nodes()
                    .iter()
                    .map(|n| n.tag.clone())
                    .collect::<Vec<_>>()
            ),
            f["tags"],
            "{name}: tags"
        );
        assert_eq!(
            ids(tree.leaves(None).unwrap()),
            f["leaves"],
            "{name}: leaves()"
        );
        assert_eq!(
            ids(tree.leaves(tree.root()).unwrap()),
            f["leaves_root"],
            "{name}: leaves(root)"
        );
        assert_eq!(
            json!(tree.expand_tree(None).unwrap()),
            f["expand_tree"],
            "{name}: expand_tree"
        );
        assert_eq!(
            ids(preorder_depth_first_search(&tree)),
            f["preorder_dfs"],
            "{name}"
        );
        assert_eq!(
            ids(preorder_breadth_first_search(&tree)),
            f["preorder_bfs"],
            "{name}"
        );
        assert_eq!(
            ids(postorder_depth_first_search(&tree)),
            f["postorder_dfs"],
            "{name}"
        );

        let sd =
            StructuresDict::new(structures.iter().map(|s| (s.clone(), None)).collect()).unwrap();
        assert_eq!(
            json!(
                sd.acronym_to_id_map()
                    .iter()
                    .map(|(a, i)| json!([a, i]))
                    .collect::<Vec<_>>()
            ),
            f["acronym_to_id"],
            "{name}: acronym_to_id_map"
        );
        assert_eq!(
            rec(sd.at_hierarchy_level("no-such-structure", Some(1))),
            f["missing"][0],
            "{name}"
        );
        assert_eq!(
            rec(sd.ancestors("no-such-structure")),
            f["missing"][1],
            "{name}"
        );

        for q in f["queries"].as_array().unwrap() {
            let id = q["id"].as_i64().unwrap();
            assert_eq!(
                rec(sd.ancestors(id)),
                q["ancestors"],
                "{name} {id}: ancestors"
            );
            if let Some(d) = q.get("descendants") {
                assert_eq!(rec(sd.descendants(id)), *d, "{name} {id}: descendants");
            }
            for (lev, want) in q["levels"].as_object().unwrap() {
                let lev = if lev == "None" {
                    None
                } else {
                    Some(lev.parse().unwrap())
                };
                assert_eq!(
                    rec(sd.at_hierarchy_level(id, lev)),
                    *want,
                    "{name} {id}: level {lev:?}"
                );
            }
            assert_eq!(
                rec(sd.at_hierarchy_level_acronyms(id, Some(1))),
                q["levels_acronym_1"],
                "{name} {id}: level 1 acronyms"
            );
        }
    }
}

#[test]
fn utils() {
    let f = load("utils.json");
    for c in f["repr"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let got = atlas_repr_from_name(name).map(|r| {
            let mut m = serde_json::Map::new();
            m.insert("name".into(), json!(r.name));
            m.insert("major_vers".into(), json!(r.major_vers));
            m.insert("minor_vers".into(), json!(r.minor_vers));
            m.insert("resolution".into(), json!(r.resolution));
            if let Some(u) = r.unit {
                m.insert("unit".into(), json!(u));
            }
            Value::Object(m)
        });
        assert_eq!(rec(got), c["r"], "atlas_repr_from_name({name})");
    }
    for c in f["from_repr"].as_array().unwrap() {
        let a = c["args"].as_array().unwrap();
        let s = |v: &Value| match v {
            Value::String(s) => Some(s.clone()),
            Value::Null => None,
            other => Some(other.to_string()),
        };
        let out = atlas_name_from_repr(
            &s(&a[0]).unwrap(),
            &s(&a[1]).unwrap(),
            a.get(2).and_then(s).as_deref(),
            a.get(3).and_then(s).as_deref(),
            a.get(4).and_then(s).as_deref().unwrap_or("um"),
        );
        assert_eq!(out, c["out"].as_str().unwrap());
    }
    for c in f["latest"].as_array().unwrap() {
        let mut v: Vec<String> = serde_json::from_value(c["in"].clone()).unwrap();
        assert_eq!(
            rec(get_latest_version(&mut v)),
            c["out"],
            "get_latest_version({:?})",
            c["in"]
        );
        if c["out"].get("ok").is_some() {
            assert_eq!(json!(v), c["sorted"], "sorted in place");
        }
    }
}

fn example_atlas() -> Atlas {
    Atlas::new(
        fixtures().join("bg/brainglobe-atlasapi/atlases/example_mouse_100um/3_0/manifest.json"),
    )
    .unwrap()
}

#[test]
fn example_atlas_metadata_and_structures() {
    let f = load("example_atlas.json");
    let a = example_atlas();
    assert_eq!(Value::Object(a.metadata.clone()), f["metadata"]);
    assert_eq!(json!(a.resolution()), f["resolution"]);
    assert_eq!(json!(a.shape()), f["shape"]);
    assert_eq!(json!(a.shape_um()), f["shape_um"]);
    assert_eq!(a.orientation(), f["orientation"]);
    assert_eq!(a.hierarchy().show(), f["hierarchy"].as_str().unwrap());
    assert_eq!(
        a.structures.to_string(),
        f["structures_repr"].as_str().unwrap()
    );
    assert_eq!(
        a.template_pyramid_level() as u64,
        f["template_level"].as_u64().unwrap()
    );
    assert_eq!(
        a.annotation_pyramid_level() as u64,
        f["annotation_level"].as_u64().unwrap()
    );
    let lookup = a.lookup_df();
    assert_eq!(
        json!(lookup.iter().map(|l| l.0.clone()).collect::<Vec<_>>()),
        f["lookup"]["acronym"]
    );
    assert_eq!(
        json!(lookup.iter().map(|l| l.1).collect::<Vec<_>>()),
        f["lookup"]["id"]
    );
    assert_eq!(
        json!(lookup.iter().map(|l| l.2.clone()).collect::<Vec<_>>()),
        f["lookup"]["name"]
    );
    let mut mapping: Vec<(i64, usize)> = a
        .annotation_mapping()
        .unwrap()
        .iter()
        .map(|(k, v)| (*k, *v))
        .collect();
    mapping.sort();
    assert_eq!(json!(mapping), f["mapping"]);
    for (id, want) in f["ancestors"].as_object().unwrap() {
        let id: i64 = id.parse().unwrap();
        assert_eq!(json!(a.get_structure_ancestors(id).unwrap()), *want);
        assert_eq!(
            json!(a.get_structure_descendants(id).unwrap()),
            f["descendants"][id.to_string()]
        );
    }
    let mf = a.meshfile_from_structure("CH").unwrap().unwrap();
    assert_eq!(
        mf.strip_prefix(&a.root_dir).unwrap().to_string_lossy(),
        f["meshfile_CH"].as_str().unwrap()
    );
}

#[test]
fn example_atlas_stacks() {
    let f = load("example_atlas.json");
    let a = example_atlas();
    for (name, arr) in [
        ("template", a.template().unwrap()),
        ("annotation", a.annotation().unwrap()),
        ("hemispheres", a.hemispheres().unwrap()),
    ] {
        let want = &f["stacks"][name];
        assert_eq!(json!(arr.shape()), want["shape"], "{name} shape");
        assert_eq!(
            arr.dtype().to_string(),
            want["dtype"].as_str().unwrap(),
            "{name} dtype"
        );
        let bytes = ome_zarr_bytes(arr);
        assert_eq!(
            sha(&bytes),
            want["sha256"].as_str().unwrap(),
            "{name} bytes"
        );
    }
}

/// C-order little-endian bytes of an array (numpy's `tobytes()`).
fn ome_zarr_bytes(a: &ome_zarr::array::NdArray) -> Vec<u8> {
    use ome_zarr::array::NdArray;
    match a {
        NdArray::UInt8(x) => x.iter().copied().collect(),
        NdArray::UInt16(x) => x.iter().flat_map(|v| v.to_le_bytes()).collect(),
        NdArray::UInt32(x) => x.iter().flat_map(|v| v.to_le_bytes()).collect(),
        other => panic!("unexpected dtype {}", other.dtype()),
    }
}

#[test]
fn example_atlas_coordinates() {
    let f = load("example_atlas.json");
    let a = example_atlas();
    for c in f["coords"].as_array().unwrap() {
        let p: Vec<f64> = serde_json::from_value(c["p"].clone()).unwrap();
        let um: Vec<f64> = serde_json::from_value(c["um"].clone()).unwrap();
        assert_eq!(
            rec(a.structure_from_coords(&p, false, None)),
            c["id"],
            "{p:?}"
        );
        assert_eq!(
            rec(a.structure_from_coords(&um, true, None)),
            c["id_um"],
            "{um:?} um"
        );
        assert_eq!(
            rec(a.structure_acronym_from_coords(&p, false, None, None)),
            c["acr"],
            "{p:?}"
        );
        assert_eq!(
            rec(a.structure_acronym_from_coords(&um, true, None, None)),
            c["acr_um"],
            "{um:?}"
        );
        assert_eq!(
            rec(a.hemisphere_from_coords(&p, false).map(|h| h as i64)),
            c["hem"],
            "{p:?}"
        );
        assert_eq!(
            rec(a.hemisphere_name_from_coords(&p, false)),
            c["hem_s"],
            "{p:?}"
        );
        for (lev, want) in c["lev"].as_object().unwrap() {
            let lev: i64 = lev.parse().unwrap();
            assert_eq!(
                rec(a.structure_from_coords(&p, false, Some(lev))),
                *want,
                "{p:?} level {lev}"
            );
        }
    }
}

#[test]
fn example_atlas_meshes_and_masks() {
    let f = load("example_atlas.json");
    let a = example_atlas();
    for (id, want) in f["meshes"].as_object().unwrap() {
        let id: i64 = id.parse().unwrap();
        let m = a.mesh_from_structure(id).unwrap().unwrap();
        assert_eq!(m.points.len() as u64, want["n_points"].as_u64().unwrap());
        assert_eq!(want["points_dtype"], "float32");
        let pts: Vec<u8> = m
            .points
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(
            sha(&pts),
            want["points_sha256"].as_str().unwrap(),
            "mesh {id} points"
        );
        let cells: Vec<u8> = m
            .cells
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(
            sha(&cells),
            want["cells_sha256"].as_str().unwrap(),
            "mesh {id} cells"
        );
    }
    for (id, want) in f["masks"].as_object().unwrap() {
        let id: i64 = id.parse().unwrap();
        let m = a.get_structure_mask(id).unwrap();
        assert_eq!(json!(m.shape()), want["shape"]);
        assert_eq!(
            sha(&ome_zarr_bytes(&m)),
            want["sha256"].as_str().unwrap(),
            "mask {id}"
        );
    }
}

#[test]
fn rich_panels() {
    let f = load("rich.json");
    for p in f["panels"].as_array().unwrap() {
        let md = p["metadata"].as_object().unwrap();
        let got = atlas_metadata_panel_at(
            p["name"].as_str().unwrap(),
            md,
            p["width"].as_u64().unwrap() as usize,
        );
        match p.get("out") {
            Some(out) => assert_eq!(
                got.unwrap(),
                out.as_str().unwrap(),
                "{} at {}",
                p["name"],
                p["width"]
            ),
            None => assert_eq!(
                rec(got),
                json!({ "err": p["err"] }),
                "{} at {}",
                p["name"],
                p["width"]
            ),
        }
    }
}

#[test]
fn rich_tables() {
    let f = load("rich.json");
    for t in f["tables"].as_array().unwrap() {
        let available: Vec<(String, String)> = t["available"]
            .as_array()
            .unwrap()
            .iter()
            .map(|kv| {
                (
                    kv[0].as_str().unwrap().to_string(),
                    kv[1].as_str().unwrap().to_string(),
                )
            })
            .collect();
        let downloaded: Vec<(String, AtlasInfo)> = t["downloaded"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, d)| {
                (
                    k.clone(),
                    AtlasInfo {
                        downloaded: true,
                        local: d["local"].as_str().unwrap().into(),
                        version: d["version"].as_str().map(String::from),
                        latest_version: d["latest_version"].as_str().unwrap().into(),
                        updated: d["updated"].as_bool(),
                    },
                )
            })
            .collect();
        let rows = table_rows(&available, &downloaded);
        let got = atlases_table_at(
            &rows,
            t["show_local_path"].as_bool().unwrap(),
            88,
            t["width"].as_u64().unwrap() as usize,
        );
        assert_eq!(
            got,
            t["out"].as_str().unwrap(),
            "table at {} ({})",
            t["width"],
            t["show_local_path"]
        );
    }
}

#[test]
fn rich_prints() {
    let f = load("rich.json");
    for p in f["prints"].as_array().unwrap() {
        let got = rprint_string(
            p["markup"].as_str().unwrap(),
            p["width"].as_u64().unwrap() as usize,
        );
        assert_eq!(
            got,
            p["out"].as_str().unwrap(),
            "{} at {}",
            p["markup"],
            p["width"]
        );
    }
}

#[test]
fn cli() {
    for c in load("cli.json").as_array().unwrap() {
        let args: Vec<String> = serde_json::from_value(c["args"].clone()).unwrap();
        let cols = c["columns"].as_u64().unwrap() as usize;
        let (stdout, stderr, code) = match parse_args(&args) {
            Parsed::Help(_) => (help_page((cols.min(80) - 2).max(50)), String::new(), 0),
            Parsed::Error(e, code) => (String::new(), e, code),
            Parsed::Run(a) => match brainglobe_atlasapi::cli::run(&a) {
                Ok(()) => (String::new(), String::new(), 0),
                Err(e) => (String::new(), format!("{}: {e}\n", e.py_type()), 1),
            },
        };
        assert_eq!(stdout, c["stdout"].as_str().unwrap(), "{args:?} stdout");
        assert_eq!(stderr, c["stderr"].as_str().unwrap(), "{args:?} stderr");
        assert_eq!(code, c["code"].as_i64().unwrap() as i32, "{args:?} code");
    }
}
