//! Parity of the configuration file with brainglobe-atlasapi (`config`) and
//! Python's `configparser`. One test: it sets `HOME` and
//! `BRAINGLOBE_CONFIG_DIR` for the whole process.

use std::path::Path;

use brainglobe_atlasapi::config;
use brainglobe_atlasapi::configparser::ConfigParser;
use serde_json::{Value, json};

fn load() -> Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/parity/config.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn config_files_and_ini() {
    let f = load();
    for text in f["ini"].as_array().unwrap() {
        let mut c = ConfigParser::new();
        c.read_string(text["in"].as_str().unwrap()).unwrap();
        let items: Vec<Value> = c
            .items()
            .into_iter()
            .map(|(n, kv)| {
                json!([
                    n,
                    kv.into_iter()
                        .map(|(k, v)| json!([k, v]))
                        .collect::<Vec<_>>()
                ])
            })
            .collect();
        assert_eq!(
            Value::Array(items),
            text["items"],
            "items of {:?}",
            text["in"]
        );
        assert_eq!(c.to_ini_string(), text["written"].as_str().unwrap());
    }

    for case in f["cases"].as_array().unwrap() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path().to_string_lossy().to_string();
        // SAFETY: this test binary has one test, so nothing reads the
        // environment concurrently.
        unsafe {
            std::env::set_var("HOME", &t);
            if case["mode"] == "env" {
                std::env::set_var("BRAINGLOBE_CONFIG_DIR", format!("{t}/cfg"));
            } else {
                std::env::remove_var("BRAINGLOBE_CONFIG_DIR");
            }
        }
        let fix = |s: &str| s.replace(&t, "<TMP>");
        let want = |k: &str| case[k].as_str().unwrap().to_string();

        assert_eq!(fix(&config::config_path().to_string_lossy()), want("path"));
        config::read_config(None).unwrap();
        assert_eq!(
            fix(&std::fs::read_to_string(config::config_path()).unwrap()),
            want("default")
        );
        assert_eq!(
            fix(&config::get_brainglobe_dir().unwrap().to_string_lossy()),
            want("dir")
        );
        let echo = |k: Option<&str>, v: Option<&str>, show: bool| {
            fix(&config::cli_modify_config(k, v, show).unwrap())
        };
        assert_eq!(echo(None, None, true), want("show"));
        assert_eq!(
            echo(Some("brainglobe_dir"), Some("/no/such/parent/x"), false),
            want("set_bad")
        );
        assert_eq!(
            echo(
                Some("brainglobe_dir"),
                Some(&format!("{t}/elsewhere")),
                false
            ),
            want("set_ok")
        );
        assert_eq!(
            echo(Some("interm_download_dir"), Some("relative"), false),
            want("set_other")
        );
        assert_eq!(
            echo(Some("missing_key"), Some("v"), false),
            want("set_nokey")
        );
        assert_eq!(
            fix(&std::fs::read_to_string(config::config_path()).unwrap()),
            want("after")
        );
    }
}
