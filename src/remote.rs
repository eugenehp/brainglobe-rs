//! The BrainGlobe bucket over https, and the web requests of
//! `brainglobe_atlasapi.utils` (feature `remote`, native only).
//!
//! [`S3FileSystem`] is the part of `s3fs.S3FileSystem(anon=True)` the API
//! uses — `exists` and `get` (a file, a directory, a `**/*.json` glob) —
//! with fsspec's rules for where copies land, so an atlas downloaded here is
//! laid out on disk exactly as Python lays it out, and either can read the
//! other's cache. Requests are anonymous S3 REST calls (`ListObjectsV2`,
//! `GET`, `HEAD`) to the bucket's public endpoint.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::configparser::ConfigParser;
use crate::descriptors;
use crate::error::{Error, Result};

/// The bucket's https endpoint (`remote_url_s3_http` without `atlas/`).
pub const BUCKET_ENDPOINT: &str = "https://brainglobe.s3.us-west-2.amazonaws.com";
/// Every key the API reads is under this prefix.
pub const BUCKET_PREFIX: &str = "atlas";

fn client(timeout: Option<Duration>) -> Result<reqwest::blocking::Client> {
    let mut b = reqwest::blocking::Client::builder()
        .user_agent(concat!("brainglobe-rs/", env!("CARGO_PKG_VERSION")));
    if let Some(t) = timeout {
        b = b.timeout(t);
    }
    b.build().map_err(|e| Error::Http(e.to_string()))
}

/// Percent-encode a key for a URL path (keys may hold spaces).
fn encode_key(key: &str) -> String {
    let mut out = String::new();
    for b in key.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn encode_query(v: &str) -> String {
    encode_key(v).replace('/', "%2F")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#13;", "\r")
        .replace("&#10;", "\n")
        .replace("&amp;", "&")
}

/// Every `<tag>…</tag>` value in a document.
fn xml_values<'a>(doc: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = vec![];
    let mut rest = doc;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i + open.len()..];
        let Some(j) = after.find(&close) else { break };
        out.push(&after[..j]);
        rest = &after[j + close.len()..];
    }
    out
}

/// An `s3://brainglobe/atlas/...` path, a bare `atlas/...` key or a path
/// relative to `atlas/` -> the key.
fn to_key(path: &str) -> String {
    let p = path.trim_start_matches("s3://brainglobe/");
    if p.starts_with("atlas/") || p == "atlas" {
        p.to_string()
    } else {
        format!("{BUCKET_PREFIX}/{}", p.trim_start_matches('/'))
    }
}

/// Anonymous read access to the BrainGlobe bucket (`s3fs.S3FileSystem(anon=True)`).
#[derive(Clone)]
pub struct S3FileSystem {
    http: reqwest::blocking::Client,
    endpoint: String,
}

impl S3FileSystem {
    /// The public bucket. `BRAINGLOBE_S3_ENDPOINT` points it elsewhere (a
    /// mirror, or a local server in tests).
    pub fn anon() -> S3FileSystem {
        S3FileSystem {
            http: client(Some(Duration::from_secs(120))).expect("http client"),
            endpoint: std::env::var("BRAINGLOBE_S3_ENDPOINT")
                .unwrap_or_else(|_| BUCKET_ENDPOINT.to_string())
                .trim_end_matches('/')
                .to_string(),
        }
    }

    fn url(&self, key: &str) -> String {
        format!("{}/{}", self.endpoint, encode_key(key))
    }

    /// Every key under `prefix` (ListObjectsV2, all pages).
    pub fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let mut keys = vec![];
        let mut token: Option<String> = None;
        loop {
            let mut url = format!(
                "{}/?list-type=2&prefix={}",
                self.endpoint,
                encode_query(prefix)
            );
            if let Some(t) = &token {
                url.push_str(&format!("&continuation-token={}", encode_query(t)));
            }
            let resp = self
                .http
                .get(&url)
                .send()
                .map_err(|e| Error::Http(e.to_string()))?;
            if !resp.status().is_success() {
                return Err(Error::Http(format!("{} listing {prefix}", resp.status())));
            }
            let body = resp.text().map_err(|e| Error::Http(e.to_string()))?;
            for k in xml_values(&body, "Key") {
                keys.push(xml_unescape(k));
            }
            let truncated = xml_values(&body, "IsTruncated").first().copied() == Some("true");
            token = xml_values(&body, "NextContinuationToken")
                .first()
                .map(|t| xml_unescape(t));
            if !truncated || token.is_none() {
                break;
            }
        }
        Ok(keys)
    }

    fn head(&self, key: &str) -> Result<bool> {
        let resp = self
            .http
            .head(self.url(key))
            .send()
            .map_err(|e| Error::Http(e.to_string()))?;
        Ok(resp.status().is_success())
    }

    /// `fs.exists(path)`: an object, or a "directory" with objects under it.
    pub fn exists(&self, path: &str) -> Result<bool> {
        let key = to_key(path);
        let key = key.trim_end_matches('/');
        if self.head(key)? {
            return Ok(true);
        }
        Ok(!self.list_one(&format!("{key}/"))?.is_empty())
    }

    fn list_one(&self, prefix: &str) -> Result<Vec<String>> {
        let url = format!(
            "{}/?list-type=2&max-keys=1&prefix={}",
            self.endpoint,
            encode_query(prefix)
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .map_err(|e| Error::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(Error::Http(format!("{} listing {prefix}", resp.status())));
        }
        let body = resp.text().map_err(|e| Error::Http(e.to_string()))?;
        Ok(xml_values(&body, "Key")
            .into_iter()
            .map(xml_unescape)
            .collect())
    }

    /// The bytes of one object.
    pub fn cat(&self, path: &str) -> Result<Vec<u8>> {
        let key = to_key(path);
        let resp = self
            .http
            .get(self.url(&key))
            .send()
            .map_err(|e| Error::Http(e.to_string()))?;
        match resp.status().as_u16() {
            200 => Ok(resp
                .bytes()
                .map_err(|e| Error::Http(e.to_string()))?
                .to_vec()),
            403 | 404 => Err(Error::file_not_found(
                descriptors::remote_url_s3(key.trim_start_matches("atlas/")).to_string(),
            )),
            s => Err(Error::Http(format!("{s} reading {key}"))),
        }
    }

    /// Download one object to `local` (its parent is created).
    pub fn get_file(&self, path: &str, local: &Path) -> Result<()> {
        let bytes = self.cat(path)?;
        if let Some(p) = local.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(local, bytes)?;
        Ok(())
    }

    /// `fs.get(rpath, lpath, recursive=...)` with fsspec's placement rules:
    ///
    /// * a file: written to `lpath` (inside it, if `lpath` is a directory);
    /// * a directory, recursively: its contents go into `lpath` when `rpath`
    ///   ends in `/` or `lpath` does not exist yet, else into
    ///   `lpath/<basename>`;
    /// * a glob (`.../**/*.json`): the matches, under `lpath`, at their path
    ///   relative to the part of the pattern before the first wildcard.
    pub fn get(&self, rpath: &str, lpath: &Path, recursive: bool) -> Result<()> {
        let key = to_key(rpath);
        if key.contains('*') || key.contains('?') || key.contains('[') {
            return self.get_glob(&key, lpath);
        }
        let trailing = key.ends_with('/');
        let base = key.trim_end_matches('/').to_string();
        if !trailing && self.head(&base)? {
            let dest = if lpath.is_dir() {
                lpath.join(base.rsplit('/').next().unwrap_or(&base))
            } else {
                lpath.to_path_buf()
            };
            return self.get_file(&base, &dest);
        }
        let keys = self.list(&format!("{base}/"))?;
        if keys.is_empty() {
            return Err(Error::file_not_found(descriptors::remote_url_s3(
                base.trim_start_matches("atlas/"),
            )));
        }
        if !recursive {
            // fsspec without recursive copies nothing from a directory
            return Ok(());
        }
        let dest_root = if trailing || !lpath.exists() {
            lpath.to_path_buf()
        } else {
            lpath.join(base.rsplit('/').next().unwrap_or(&base))
        };
        let jobs: Vec<(String, PathBuf)> = keys
            .into_iter()
            .filter(|k| !k.ends_with('/'))
            .map(|k| {
                let rel = k[base.len() + 1..].to_string();
                (k, dest_root.join(rel))
            })
            .collect();
        std::fs::create_dir_all(&dest_root)?;
        self.get_many(jobs)
    }

    fn get_glob(&self, pattern: &str, lpath: &Path) -> Result<()> {
        let wild = pattern.find(['*', '?', '[']).unwrap();
        let root = match pattern[..wild].rfind('/') {
            Some(i) => &pattern[..i],
            None => "",
        };
        let keys = self.list(&format!("{root}/"))?;
        let jobs: Vec<(String, PathBuf)> = keys
            .into_iter()
            .filter(|k| glob_match(pattern, k))
            .map(|k| {
                let rel = k[root.len() + 1..].to_string();
                (k, lpath.join(rel))
            })
            .collect();
        if jobs.is_empty() {
            return Err(Error::file_not_found(descriptors::remote_url_s3(
                pattern.trim_start_matches("atlas/"),
            )));
        }
        self.get_many(jobs)
    }

    /// Download many objects, 16 at a time.
    fn get_many(&self, jobs: Vec<(String, PathBuf)>) -> Result<()> {
        let workers = 16.min(jobs.len()).max(1);
        let queue = std::sync::Mutex::new(jobs.into_iter());
        let err: std::sync::Mutex<Option<Error>> = std::sync::Mutex::new(None);
        std::thread::scope(|s| {
            for _ in 0..workers {
                s.spawn(|| {
                    loop {
                        if err.lock().unwrap().is_some() {
                            return;
                        }
                        let Some((k, dest)) = queue.lock().unwrap().next() else {
                            return;
                        };
                        if let Err(e) = self.get_file(&k, &dest) {
                            *err.lock().unwrap() = Some(e);
                            return;
                        }
                    }
                });
            }
        });
        match err.into_inner().unwrap() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// fsspec's glob: `**` any number of path components (zero included), `*`
/// and `?` within one component.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    fn seg_match(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(b'*'), _) => seg_match(&p[1..], s) || (!s.is_empty() && seg_match(p, &s[1..])),
            (Some(b'?'), Some(_)) => seg_match(&p[1..], &s[1..]),
            (Some(a), Some(b)) if a == b => seg_match(&p[1..], &s[1..]),
            _ => false,
        }
    }
    fn parts_match(p: &[&str], s: &[&str]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some(&"**") => (0..=s.len()).any(|k| parts_match(&p[1..], &s[k..])),
            Some(seg) => {
                !s.is_empty()
                    && seg_match(seg.as_bytes(), s[0].as_bytes())
                    && parts_match(&p[1..], &s[1..])
            }
        }
    }
    let p: Vec<&str> = pattern.split('/').collect();
    let s: Vec<&str> = path.split('/').collect();
    parts_match(&p, &s)
}

/// Check that there is an internet connection (`check_internet_connection`):
/// google, then pypi, bing, gitee and perdu.com.
pub fn check_internet_connection(
    url: Option<&str>,
    timeout: u64,
    raise_error: bool,
) -> Result<bool> {
    let url = url.unwrap_or("http://www.google.com/");
    let mut urls = vec![url.to_string()];
    if url == "http://www.google.com/" {
        for u in [
            "https://pypi.org/",
            "https://www.bing.com/",
            "https://gitee.com/",
            "http://perdu.com/",
        ] {
            if u != url {
                urls.push(u.to_string());
            }
        }
    }
    let http = client(Some(Duration::from_secs(timeout)))?;
    for u in &urls {
        if http.get(u).send().is_ok() {
            return Ok(true);
        }
    }
    if !raise_error {
        println!("No internet connection available.");
        Ok(false)
    } else {
        Err(Error::Connection(
            "No internet connection, try again when you are connected to the internet.".into(),
        ))
    }
}

/// Check that the BrainGlobe bucket is reachable (`check_s3_status`).
pub fn check_s3_status(timeout: u64, raise_error: bool) -> Result<bool> {
    let url = descriptors::remote_url_s3_http("");
    let http = client(Some(Duration::from_secs(timeout)))?;
    match http.head(&url).send() {
        Ok(_) => Ok(true),
        Err(_) => {
            let msg = "S3 BrainGlobe bucket is not accessible.";
            if !raise_error {
                println!("{msg}");
                Ok(false)
            } else {
                Err(Error::Connection(msg.into()))
            }
        }
    }
}

/// Read an INI file from a URL, caching a copy (`conf_from_url`): five
/// tries, half a second apart; on failure, the cached copy.
pub fn conf_from_url(url: &str, cache_path: &Path) -> Result<ConfigParser> {
    let http = client(Some(Duration::from_secs(60)))?;
    let mut status = 418u16; // teapot
    let mut text = None;
    for _ in 0..5 {
        if let Ok(r) = http.get(url).send() {
            status = r.status().as_u16();
            if status == 200 {
                text = r.text().ok();
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let Some(text) = text.filter(|_| status == 200) else {
        println!("Could not fetch the latest atlas versions: {status}");
        println!(
            "Using the last cached version from {}",
            cache_path.display()
        );
        return crate::utils::conf_from_file(cache_path);
    };
    let mut config = ConfigParser::new();
    config.read_string(&text)?;
    let write = || -> std::io::Result<()> {
        if let Some(p) = cache_path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(cache_path, config.to_ini_string())
    };
    if let Err(e) = write() {
        println!("Could not update the latest atlas versions cache: {e}");
    }
    Ok(config)
}

/// Download a file over http (`retrieve_over_http`); `fn_update` gets the
/// bytes done and the total after each chunk.
pub fn retrieve_over_http(
    url: &str,
    output_file_path: &Path,
    mut fn_update: Option<&mut dyn FnMut(u64, u64)>,
) -> Result<()> {
    use std::io::{Read, Write};
    let http = client(None)?;
    let mut run = || -> Result<()> {
        let mut resp = http
            .get(url)
            .send()
            .map_err(|_| Error::Connection(format!("Could not download file from {url}")))?;
        let tot = resp.content_length().unwrap_or(0);
        let mut f = std::fs::File::create(output_file_path)?;
        let mut buf = [0u8; 4096];
        let mut completed = 0u64;
        loop {
            let n = resp
                .read(&mut buf)
                .map_err(|_| Error::Connection(format!("Could not download file from {url}")))?;
            if n == 0 {
                break;
            }
            f.write_all(&buf[..n])?;
            completed += n as u64;
            if let Some(cb) = fn_update.as_deref_mut() {
                cb(completed, tot);
            }
        }
        Ok(())
    };
    let r = run();
    if matches!(r, Err(Error::Connection(_))) {
        let _ = std::fs::remove_file(output_file_path);
    }
    r
}
