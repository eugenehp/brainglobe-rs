#!/usr/bin/env python3
"""Reference outputs of brainglobe-atlasapi for the Rust parity tests.

Run in an environment with brainglobe-atlasapi installed (at the commit in
README.md); it downloads what it needs from the BrainGlobe bucket into a
private BrainGlobe directory, never the user's:

    python -I scripts/gen_parity_fixtures.py [--out tests/fixtures]

Writes
    tests/fixtures/terminology/<name>.csv   real terminologies (inputs)
    tests/fixtures/bg/brainglobe-atlasapi/  example_mouse_100um, complete
    tests/fixtures/parity/*.json            what Python makes of them
"""
import argparse
import contextlib
import hashlib
import io
import json
import os
import random
import shutil
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
BUCKET = "https://brainglobe.s3.us-west-2.amazonaws.com/atlas"

TERMINOLOGIES = {
    "example_mouse": "3_0", "allen_mouse": "3_0", "whs_sd_rat": "3_0",
    "unam_axolotl": "3_0", "azba_zfish": "3_0", "mpin_zfish": "3_0",
    "kim_mouse": "3_0", "nmt_arm_sym_macaque": "3_0", "allen_human": "3_1",
    "allen-adult-human": "2016", "csl_cat": "3_0", "columbia_cuttlefish": "3_0",
    "kocher_bumblebee": "3_0", "vfb_jrc2018u_neuropils_fly": "3_0",
    "prairie_vole": "3_0", "eurasian_blackcap": "3_0", "demba-p10-mouse": "3_0",
    "duke_mouse": "3_0", "perens_lsfm_mouse": "3_0", "african_molerat": "3_0",
    "admba-p56-mouse": "3_0", "sju_cavefish": "3_0",
    "danionella_cerebrum_mixed": "3_0", "dorr_mouse_mri": "3_0",
    "hoops_tawny_dragon": "3_0", "drosophila_wingdisc_instar3": "3_0",
}


def fetch(url):
    with urllib.request.urlopen(url, timeout=120) as r:
        return r.read()


def sha(b):
    return hashlib.sha256(b).hexdigest()


def err(e):
    return {"type": type(e).__name__, "msg": str(e)}


def call(f, *a, **k):
    try:
        return {"ok": f(*a, **k)}
    except Exception as e:  # noqa: BLE001 - every error is recorded
        return {"err": err(e)}


def jsonable(v):
    import numpy as np
    if isinstance(v, dict):
        return {k: jsonable(x) for k, x in v.items()}
    if isinstance(v, (list, tuple)):
        return [jsonable(x) for x in v]
    if isinstance(v, np.generic):
        return v.item()
    if isinstance(v, float) and v != v:
        return None
    if v is None or isinstance(v, (bool, int, float, str)):
        return v
    try:
        import pandas as pd
        if v is pd.NA:
            return None
    except Exception:
        pass
    return str(v)


def dump(path, obj):
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w") as f:
        json.dump(obj, f, indent=1, ensure_ascii=False)
    print("wrote", path, f"{path.stat().st_size / 1e3:.0f} kB")


def terminology_fixtures(out):
    from brainglobe_atlasapi import core
    from brainglobe_atlasapi.structure_class import StructuresDict
    from brainglobe_atlasapi.structure_tree_util import (
        get_structures_tree, postorder_depth_first_search,
        preorder_breadth_first_search, preorder_depth_first_search)
    from brainglobe_atlasapi.utils import load_structures_from_csv

    class Fake(core.Atlas):
        def __init__(self, structures_list):  # noqa: D107 - structures only
            self.structures_list = structures_list
            self.structures = StructuresDict(structures_list)

    result = {}
    rng = random.Random(0)
    for name, version in TERMINOLOGIES.items():
        csv_path = out / "terminology" / f"{name}.csv"
        csv_path.parent.mkdir(parents=True, exist_ok=True)
        if not csv_path.exists():
            csv_path.write_bytes(fetch(f"{BUCKET}/terminologies/{name}-terminology/{version}/terminology.csv"))
        structures = load_structures_from_csv(csv_path)
        for s in structures:
            s["mesh_filename"] = None
        tree = get_structures_tree(structures)
        fake = Fake(structures)
        ids = [s["id"] for s in structures]
        sample = ids if len(ids) <= 60 else sorted(rng.sample(ids, 60), key=ids.index)
        maxdepth = max(len(s["structure_id_path"]) for s in structures)
        queries = []
        for sid in sample:
            acr = fake.structures[sid]["acronym"]
            q = {"id": sid, "acronym": jsonable(acr),
                 "ancestors": call(fake.get_structure_ancestors, sid),
                 "levels": {}}
            if len(ids) <= 400 or len(queries) < 15:
                q["descendants"] = call(fake.get_structure_descendants, sid)
            for lev in [None, *range(0, maxdepth + 1), -1]:
                q["levels"][str(lev)] = call(fake.get_structures_at_hierarchy_level, sid, lev)
            q["levels_acronym_1"] = call(fake.get_structures_at_hierarchy_level, sid, 1, as_acronym=True)
            queries.append(jsonable(q))
        result[name] = {
            "structures": jsonable([{k: v for k, v in s.items() if k != "mesh_filename"} for s in structures]),
            "show": tree.show(stdout=False),
            "nodes": list(tree.nodes.keys()),
            "tags": [tree.nodes[k].tag for k in tree.nodes],
            "leaves": [n.identifier for n in tree.leaves()],
            "leaves_root": [n.identifier for n in tree.leaves(tree.root)],
            "expand_tree": list(tree.expand_tree()),
            "preorder_dfs": [n.identifier for n in preorder_depth_first_search(tree)],
            "preorder_bfs": [n.identifier for n in preorder_breadth_first_search(tree)],
            "postorder_dfs": [n.identifier for n in postorder_depth_first_search(tree)],
            "acronym_to_id": jsonable(list(fake.structures.acronym_to_id_map.items())),
            "missing": [call(fake.get_structures_at_hierarchy_level, "no-such-structure", 1),
                        call(fake.get_structure_ancestors, "no-such-structure")],
            "queries": queries,
        }
    dump(out / "parity" / "terminology.json", result)


def pyset_fixtures(out):
    rng = random.Random(1)
    cases = []
    for n in [0, 1, 2, 5, 7, 8, 9, 20, 50, 100, 300, 1000, 3000]:
        for hi in [10, 1000, 10**6, 10**9, 2**40]:
            xs = [rng.randrange(0, hi) for _ in range(n)]
            cases.append({"in": xs, "out": list(set(xs))})
    cases.append({"in": [-1, -2, -3, 5, -(2**62), 2**62], "out": list(set([-1, -2, -3, 5, -(2**62), 2**62]))})
    dump(out / "parity" / "pyset.json", cases)


def utils_fixtures(out):
    from brainglobe_atlasapi import atlas_name, utils
    names = list(atlas_name.AtlasName.__args__)
    extra = ["allen_mouse_25um_v1.2", "x_10nm", "x_1mm_v0.1", "a_b", "nounit",
             "a_25um_v1", "a_25um_v1.2.3", "_um", "a__b_3um"]
    repr_cases = [{"name": n, "r": call(utils.atlas_repr_from_name, n)} for n in names + extra]
    from_repr = []
    for args in [("allen_mouse", 25), ("allen_mouse", 25, 1, 2), ("x", "1.5", None, None, "mm"),
                 ("x", 3, 0, None), ("x", 3, None, 7)]:
        from_repr.append({"args": [jsonable(a) for a in args], "out": utils.atlas_name_from_repr(*args)})
    latest = []
    for vs in [["3_0"], ["3_0", "3_1", "2_9"], ["10_0", "9_9"], ["1", "1_0", "0_99"],
               ["3_0", "3_0"], [], ["a_1"], ["1_x"], ["01_2", "1_10"], [" 2_1", "2_1 "]]:
        lst = list(vs)
        r = call(utils.get_latest_version, lst)
        latest.append({"in": vs, "out": r, "sorted": lst})
    dump(out / "parity" / "utils.json", {"repr": jsonable(repr_cases), "from_repr": from_repr, "latest": latest})


def example_atlas_fixtures(out, bg_root):
    """Everything about example_mouse_100um, downloading every lazy piece so
    the fixture directory is complete."""
    import numpy as np
    from brainglobe_atlasapi.bg_atlas import BrainGlobeAtlas
    a = BrainGlobeAtlas("example_mouse_100um", check_latest=False, brainglobe_dir=bg_root)
    res = {}
    res["metadata"] = a.metadata
    res["resolution"] = list(a.resolution)
    res["shape"] = list(a.shape)
    res["shape_um"] = list(a.shape_um)
    res["orientation"] = a.orientation
    res["repr"] = repr(a)
    res["lookup"] = jsonable(a.lookup_df.to_dict(orient="list"))
    res["hierarchy"] = a.hierarchy.show(stdout=False)
    res["structures_repr"] = repr(a.structures)
    res["template_level"] = a._template_pyramid_level
    res["annotation_level"] = a._annotation_pyramid_level
    res["mapping"] = jsonable(sorted(a._annotation_mapping.items()))
    stacks = {}
    for name in ["template", "annotation", "hemispheres"]:
        arr = np.ascontiguousarray(getattr(a, name))
        stacks[name] = {"shape": list(arr.shape), "dtype": str(arr.dtype), "sha256": sha(arr.tobytes())}
    res["stacks"] = stacks
    coords = []
    rng = random.Random(2)
    shape = a.shape
    pts = [[39, 36, 57], [1, 1, 1], [0, 0, 0], [65, 39, 56], [131, 79, 113], [-1, -1, -1],
           [132, 0, 0], [0, 80, 0], [10.7, 20.2, 30.9], [-0.5, 3, 3]]
    pts += [[rng.uniform(0, s) for s in shape] for _ in range(300)]
    for p in pts:
        microns = [c * r for c, r in zip(p, a.resolution)]
        c = {"p": p, "um": microns,
             "id": call(lambda: int(a.structure_from_coords(p))),
             "id_um": call(lambda: int(a.structure_from_coords(microns, microns=True))),
             "acr": call(a.structure_from_coords, p, as_acronym=True),
             "acr_um": call(a.structure_from_coords, microns, microns=True, as_acronym=True),
             "hem": call(lambda: int(a.hemisphere_from_coords(p))),
             "hem_s": call(a.hemisphere_from_coords, p, as_string=True),
             "lev": {}}
        for lev in [0, 1, 2, 3, -1, 9]:
            c["lev"][str(lev)] = call(lambda: int(a.structure_from_coords(p, hierarchy_lev=lev)))
        coords.append(jsonable(c))
    res["coords"] = coords
    meshes = {}
    for sid in a.structures.keys():
        m = a.mesh_from_structure(sid)
        pts_ = np.ascontiguousarray(m.points)
        cells = np.ascontiguousarray(m.cells[0].data)
        meshes[str(sid)] = {"n_points": len(pts_), "points_dtype": str(pts_.dtype),
                            "points_sha256": sha(pts_.tobytes()), "first": pts_[0].tolist(),
                            "cells_dtype": str(cells.dtype), "n_cells": len(cells),
                            "cells_sha256": sha(cells.astype(np.uint32).tobytes())}
    res["meshes"] = meshes
    masks = {}
    for sid in a.structures.keys():
        m = np.ascontiguousarray(a.get_structure_mask(sid))
        masks[str(sid)] = {"shape": list(m.shape), "dtype": str(m.dtype), "sha256": sha(m.tobytes()),
                           "sum": int(m.sum())}
    res["masks"] = masks
    res["ancestors"] = {str(s): a.get_structure_ancestors(s) for s in a.structures.keys()}
    res["descendants"] = {str(s): a.get_structure_descendants(s) for s in a.structures.keys()}
    res["meshfile_CH"] = str(Path(a.meshfile_from_structure("CH")).relative_to(a.root_dir))
    dump(out / "parity" / "example_atlas.json", jsonable(res))


def rich_fixtures(out):
    from rich.console import Console
    from brainglobe_atlasapi import list_atlases
    from brainglobe_atlasapi.utils import _rich_atlas_metadata

    keys = []
    token = None
    while True:
        url = f"{BUCKET.rsplit('/', 1)[0]}/?list-type=2&prefix=atlas/atlases/"
        if token:
            url += "&continuation-token=" + urllib.parse.quote(token, safe="")
        body = fetch(url).decode()
        keys += [k.split("</Key>")[0] for k in body.split("<Key>")[1:]]
        if "<IsTruncated>true" not in body:
            break
        token = body.split("<NextContinuationToken>")[1].split("<")[0]
    manifests = [k for k in keys if k.endswith("/manifest.json")]
    panels = []
    for k in manifests:
        md = json.loads(fetch(f"{BUCKET.rsplit('/', 1)[0]}/{k}"))
        name = k.split("/")[2]
        for w in [40, 60, 80, 100, 140]:
            buf = io.StringIO()
            try:
                Console(file=buf, width=w, force_terminal=False, color_system=None).print(
                    _rich_atlas_metadata(name, md))
                panels.append({"name": name, "metadata": md, "width": w, "out": buf.getvalue()})
            except Exception as e:  # noqa: BLE001
                panels.append({"name": name, "metadata": md, "width": w, "err": err(e)})

    rng = random.Random(3)
    conf = fetch(f"{BUCKET}/atlases/last_versions.conf").decode()
    available = {}
    for line in conf.splitlines():
        if "=" in line:
            kk, vv = line.split("=", 1)
            available[kk.strip()] = vv.strip()
    tables = []
    for case in range(6):
        names = list(available)
        down = rng.sample(names, rng.randrange(0, 6))
        downloaded = {}
        for n in down:
            v = available[n] if rng.random() < 0.6 else rng.choice(["2.0", "1.2", "3.0-rc"])
            downloaded[n] = dict(downloaded=True, local=f"/tmp/bg/{n}", version=v,
                                 latest_version=available[n], updated=v == available[n])
        for w in [60, 80, 88, 120]:
            for show in [False, True]:
                buf = io.StringIO()
                import rich
                rich.reconfigure(file=buf, width=w, force_terminal=False, color_system=None)
                orig = (list_atlases.get_all_atlases_lastversions, list_atlases.get_atlases_lastversions)
                list_atlases.get_all_atlases_lastversions = lambda: available
                list_atlases.get_atlases_lastversions = lambda: downloaded
                try:
                    list_atlases.show_atlases(show_local_path=show)
                finally:
                    (list_atlases.get_all_atlases_lastversions, list_atlases.get_atlases_lastversions) = orig
                tables.append({"available": list(available.items()), "downloaded": downloaded,
                               "width": w, "show_local_path": show, "out": buf.getvalue()})

    prints = []
    msgs = [
        "[b][magenta2]brainglobe_atlasapi[/b]: [b]{n}[/b] version [b]2.0[/b] is not the latest available ([b]3.1[/b]). To update the atlas run in the terminal:[/magenta2]\n [gold1]brainglobe update -a {n}[/gold1]",
        "[b][magenta2]brainglobe_atlasapi: {n} is already updated (version: 3_0)[/b]",
        "[b][magenta2]brainglobe_atlasapi: updating {n}[/magenta2][/b]",
        "[b][magenta2]brainglobe_atlasapi: installing {n}: atlas already installed![/magenta2][/b]",
        "plain \\[not a tag] [1] [#fff]x[/] :heavy_check_mark: :nope: a:b:c",
    ]
    import rich
    for n in ["example_mouse_100um", "demba_allen_seg_dev_mouse_p10_20um", "x" * 90]:
        for m in msgs:
            for w in [20, 40, 80, 120]:
                buf = io.StringIO()
                rich.reconfigure(file=buf, width=w, force_terminal=False, color_system=None)
                rich.print(m.format(n=n))
                prints.append({"markup": m.format(n=n), "width": w, "out": buf.getvalue()})
    dump(out / "parity" / "rich.json", {"panels": panels, "tables": tables, "prints": prints})


CONFIG_SCRIPT = r"""
import contextlib, io, json, sys
from brainglobe_atlasapi import config
out = {}
out["path"] = str(config.CONFIG_PATH)
conf = config.read_config()
out["default"] = open(config.CONFIG_PATH).read()
out["dir"] = str(config.get_brainglobe_dir())
def echo(**k):
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        try:
            config.cli_modify_config(**k)
        except Exception as e:
            print("EXC", type(e).__name__, e)
    return buf.getvalue()
out["show"] = echo(show=True)
out["set_bad"] = echo(key="brainglobe_dir", value="/no/such/parent/x")
out["set_ok"] = echo(key="brainglobe_dir", value=sys.argv[1] + "/elsewhere")
out["set_other"] = echo(key="interm_download_dir", value="relative")
out["set_nokey"] = echo(key="missing_key", value="v")
out["after"] = open(config.CONFIG_PATH).read()
print(json.dumps(out))
"""


def config_fixtures(out):
    cases = []
    for mode in ["home", "env"]:
        with tempfile.TemporaryDirectory() as tmp:
            env = {k: v for k, v in os.environ.items() if k not in ("BRAINGLOBE_CONFIG_DIR",)}
            env["HOME"] = tmp
            if mode == "env":
                env["BRAINGLOBE_CONFIG_DIR"] = tmp + "/cfg"
            r = subprocess.run([sys.executable, "-I", "-c", CONFIG_SCRIPT, tmp], env=env,
                               capture_output=True, text=True, check=True)
            d = json.loads(r.stdout)
            cases.append({k: (v.replace(tmp, "<TMP>") if isinstance(v, str) else v) for k, v in d.items()} | {"mode": mode})
    ini = ["[a]\nX = 1\ny: two\n\n[b]\nlong = line one\n  line two\n\n# c\n; d\nz=\n",
           "[DEFAULT]\nd = 1\n[s]\nk = v\n", "[atlases]\nallen_mouse_25um = 3.1\n"]
    from configparser import ConfigParser
    parsed = []
    for text in ini:
        c = ConfigParser()
        c.read_string(text)
        buf = io.StringIO()
        c.write(buf)
        parsed.append({"in": text, "items": [[n, list(s.items())] for n, s in c.items()], "written": buf.getvalue()})
    dump(out / "parity" / "config.json", {"cases": cases, "ini": parsed})


CLI_CASES = ["-h", "--help", "", "--sho", "-x", "list extra", "-a", "list --atlas_name",
             "--atlas-name x list", "--show=1 list", "a b c", "--at x", "-sk", "-fa", "list -h",
             "-x -h", "-h -x", "--", "-- -s", "--key", "--valu 3 x", "--forc", "-a x -a y bogus",
             "config --key", "bogus", "install", "update"]


def cli_fixtures(out):
    cases = []
    with tempfile.TemporaryDirectory() as tmp:
        env = {k: v for k, v in os.environ.items() if k != "BRAINGLOBE_CONFIG_DIR"}
        env["HOME"] = tmp
        exe = str(Path(sys.executable).with_name("brainglobe"))
        for args in CLI_CASES:
            for cols in ["80", "50", "120"]:
                if cols != "80" and args not in ("-h",):
                    continue
                env["COLUMNS"] = cols
                r = subprocess.run([exe, *args.split()], env=env, capture_output=True, text=True)
                err_ = r.stderr
                if "Traceback" in err_:
                    err_ = err_.strip().splitlines()[-1] + "\n"
                cases.append({"args": args.split(), "columns": int(cols), "stdout": r.stdout,
                              "stderr": err_, "code": r.returncode})
    dump(out / "parity" / "cli.json", cases)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=str(HERE.parent / "tests" / "fixtures"))
    ap.add_argument("--only", nargs="*")
    args = ap.parse_args()
    out = Path(args.out)
    bg_root = out / "bg"
    # never the user's BrainGlobe directory
    cfg = Path(tempfile.mkdtemp())
    (cfg / "bg_config.conf").write_text(f"[default_dirs]\nbrainglobe_dir = {bg_root}\ninterm_download_dir = {bg_root}\n\n")
    os.environ["BRAINGLOBE_CONFIG_DIR"] = str(cfg)
    import brainglobe_atlasapi.config as c
    c.CONFIG_DIR = cfg
    c.CONFIG_PATH = cfg / "bg_config.conf"
    steps = {"terminology": lambda: terminology_fixtures(out), "pyset": lambda: pyset_fixtures(out),
             "utils": lambda: utils_fixtures(out), "example": lambda: example_atlas_fixtures(out, bg_root),
             "rich": lambda: rich_fixtures(out), "config": lambda: config_fixtures(out),
             "cli": lambda: cli_fixtures(out)}
    for name, f in steps.items():
        if args.only and name not in args.only:
            continue
        print("==", name)
        f()
    shutil.rmtree(cfg, ignore_errors=True)


if __name__ == "__main__":
    import urllib.parse  # noqa: F401
    main()
