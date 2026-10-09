//! Loading a Bruno collection directory into a lightweight tree.
//!
//! The tree only keeps [`RequestSummary`] values (name, method, seq, path):
//! full requests are parsed on demand when a tab is opened. This keeps memory
//! flat for large collections while still making the sidebar and command
//! palette instantaneous.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bru::{self, Block, BruFile};
use crate::env::{Environment, VarScope};
use crate::error::CoreError;
use crate::model::{HttpMethod, HttpRequest, KeyValue};

pub const COLLECTION_MANIFEST: &str = "bruno.json";
pub const COLLECTION_FILE: &str = "collection.bru";
pub const FOLDER_FILE: &str = "folder.bru";
pub const ENVIRONMENTS_DIR: &str = "environments";
/// Block of `folder.bru` / `collection.bru` holding the variables that every
/// request beneath that folder (or the whole collection) can use.
pub const FOLDER_VARS_BLOCK: &str = "vars:pre-request";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestSummary {
    pub name: String,
    pub method: HttpMethod,
    pub seq: Option<u32>,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub name: String,
    pub seq: Option<u32>,
    pub path: PathBuf,
    /// `vars:pre-request` of this folder's `folder.bru` (or of
    /// `collection.bru` for the root).
    pub vars: Vec<KeyValue>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Node {
    Folder(Folder),
    Request(RequestSummary),
}

impl Node {
    pub fn name(&self) -> &str {
        match self {
            Node::Folder(f) => &f.name,
            Node::Request(r) => &r.name,
        }
    }

    fn sort_key(&self) -> (u8, u32, &str) {
        match self {
            Node::Folder(f) => (0, f.seq.unwrap_or(u32::MAX), &f.name),
            Node::Request(r) => (1, r.seq.unwrap_or(u32::MAX), &r.name),
        }
    }
}

/// A file that failed to load; surfaced in the UI instead of aborting.
#[derive(Debug)]
pub struct LoadIssue {
    pub path: PathBuf,
    pub error: CoreError,
}

#[derive(Debug)]
pub struct Collection {
    pub name: String,
    pub root: Folder,
    pub environments: Vec<Environment>,
    pub issues: Vec<LoadIssue>,
}

impl Collection {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CoreError> {
        let path = path.as_ref();
        let dir_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let name = read_manifest_name(path).unwrap_or(dir_name);

        let mut issues = Vec::new();
        let root = load_folder(path, name.clone(), None, true, &mut issues)?;
        let environments = load_environments(&path.join(ENVIRONMENTS_DIR), &mut issues);

        Ok(Self {
            name,
            root,
            environments,
            issues,
        })
    }

    /// Depth-first iterator over every request in sidebar order.
    pub fn requests(&self) -> impl Iterator<Item = &RequestSummary> {
        let mut stack: Vec<std::slice::Iter<'_, Node>> = vec![self.root.children.iter()];
        std::iter::from_fn(move || {
            while let Some(top) = stack.last_mut() {
                match top.next() {
                    Some(Node::Request(r)) => return Some(r),
                    Some(Node::Folder(f)) => stack.push(f.children.iter()),
                    None => {
                        stack.pop();
                    }
                }
            }
            None
        })
    }

    pub fn environment(&self, name: &str) -> Option<&Environment> {
        self.environments.iter().find(|e| e.name == name)
    }

    /// The folder at `path`, if it is part of the tree (the root included).
    pub fn folder(&self, path: &Path) -> Option<&Folder> {
        self.folder_chain(path)
            .last()
            .copied()
            .filter(|f| f.path == path)
    }

    /// The root followed by every folder down to the one containing `path`.
    fn folder_chain(&self, path: &Path) -> Vec<&Folder> {
        let mut chain = vec![&self.root];
        while let Some(next) = chain.last().and_then(|f| {
            f.children.iter().find_map(|n| match n {
                Node::Folder(child) if path.starts_with(&child.path) => Some(child),
                _ => None,
            })
        }) {
            chain.push(next);
        }
        chain
    }

    /// Variables visible to the request (or folder) at `path`, following
    /// Bruno's precedence: deeper folders > shallower folders > environment >
    /// collection. Request vars are layered on top by `davi_net::prepare`.
    ///
    /// Folder values are resolved against the layers below them, so a folder
    /// can extend an inherited value (`baseUrl: {{baseUrl}}/v2`).
    pub fn var_scope(&self, path: &Path, environment: Option<&Environment>) -> VarScope {
        let enabled = |vars: &[KeyValue]| -> Vec<(String, String)> {
            vars.iter()
                .filter(|v| v.enabled)
                .map(|v| (v.name.clone(), v.value.clone()))
                .collect()
        };
        let mut scope = VarScope::new();
        if let Some(env) = environment {
            scope.push_layer(env.enabled_vars());
        }
        scope.push_layer(enabled(&self.root.vars));

        for folder in self.folder_chain(path).into_iter().skip(1) {
            if !folder.vars.iter().any(|v| v.enabled) {
                continue;
            }
            let resolved: Vec<(String, String)> = enabled(&folder.vars)
                .into_iter()
                .map(|(k, v)| {
                    let v = scope.interpolate(&v).into_owned();
                    (k, v)
                })
                .collect();
            let mut layered = VarScope::new().with_layer(resolved);
            layered.extend_from(&scope);
            scope = layered;
        }
        scope
    }
}

/// Read and fully parse a request file.
pub fn load_request(path: &Path) -> Result<HttpRequest, CoreError> {
    let source = std::fs::read_to_string(path).map_err(|e| CoreError::io(path, e))?;
    bru::parse_request(&source).map_err(|e| e.in_file(path))
}

/// Serialize and atomically write a request file (write + rename), so a
/// crash mid-save never leaves a truncated `.bru` behind.
pub fn save_request(path: &Path, request: &HttpRequest) -> Result<(), CoreError> {
    write_atomic(path, &bru::write_request(request))
}

fn write_atomic(path: &Path, text: &str) -> Result<(), CoreError> {
    let tmp = path.with_extension("bru.tmp");
    std::fs::write(&tmp, text).map_err(|e| CoreError::io(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| CoreError::io(path, e))
}

// ---------------------------------------------------------------------------
// Environments and folder variables
// ---------------------------------------------------------------------------

/// `<root>/environments/<name>.bru`. The file stem is the environment name.
pub fn environment_path(root: &Path, name: &str) -> PathBuf {
    root.join(ENVIRONMENTS_DIR)
        .join(format!("{}.bru", sanitize_file_name(name)))
}

/// Write `environment` to its file, creating `environments/` if needed.
/// Secret values are not written (see [`Environment::to_bru`]).
pub fn save_environment(root: &Path, environment: &Environment) -> Result<PathBuf, CoreError> {
    let dir = root.join(ENVIRONMENTS_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
    let path = environment_path(root, &environment.name);
    write_atomic(&path, &bru::write(&environment.to_bru()))?;
    Ok(path)
}

pub fn delete_environment(root: &Path, name: &str) -> Result<(), CoreError> {
    let path = environment_path(root, name);
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(CoreError::io(&path, e)),
        _ => Ok(()),
    }
}

/// The file holding a folder's settings: `collection.bru` for the collection
/// root, `folder.bru` otherwise.
pub fn folder_settings_file(root: &Path, dir: &Path) -> PathBuf {
    dir.join(if dir == root {
        COLLECTION_FILE
    } else {
        FOLDER_FILE
    })
}

/// Replace the variables of the folder `dir` (or of the collection when
/// `dir == root`), keeping every other block of its settings file.
pub fn save_folder_vars(root: &Path, dir: &Path, vars: &[KeyValue]) -> Result<(), CoreError> {
    let path = folder_settings_file(root, dir);
    let mut file = if path.exists() {
        let source = std::fs::read_to_string(&path).map_err(|e| CoreError::io(&path, e))?;
        bru::parse(&source).map_err(|e| CoreError::from(e).in_file(&path))?
    } else if vars.is_empty() {
        return Ok(());
    } else {
        BruFile::default()
    };

    if dir != root && file.block("meta").is_none() {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        file.blocks
            .insert(0, Block::dict("meta", vec![KeyValue::new("name", name)]));
    }
    let block = Block::dict(FOLDER_VARS_BLOCK, vars.to_vec());
    match file.blocks.iter().position(|b| b.name == FOLDER_VARS_BLOCK) {
        Some(ix) if vars.is_empty() => {
            file.blocks.remove(ix);
        }
        Some(ix) => file.blocks[ix] = block,
        None if !vars.is_empty() => file.blocks.push(block),
        None => {}
    }
    write_atomic(&path, &bru::write(&file))
}

fn read_manifest_name(dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join(COLLECTION_MANIFEST)).ok()?;
    let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
    json.get("name")?.as_str().map(str::to_owned)
}

fn should_skip_dir(name: &str, is_root: bool) -> bool {
    name.starts_with('.') || name == "node_modules" || (is_root && name == ENVIRONMENTS_DIR)
}

fn load_folder(
    path: &Path,
    default_name: String,
    default_seq: Option<u32>,
    is_root: bool,
    issues: &mut Vec<LoadIssue>,
) -> Result<Folder, CoreError> {
    let mut folder = Folder {
        name: default_name,
        seq: default_seq,
        path: path.to_path_buf(),
        vars: Vec::new(),
        children: Vec::new(),
    };

    let entries = std::fs::read_dir(path).map_err(|e| CoreError::io(path, e))?;
    for entry in entries.flatten() {
        let entry_path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_dir() {
            if should_skip_dir(&file_name, is_root) {
                continue;
            }
            match load_folder(&entry_path, file_name.into_owned(), None, false, issues) {
                Ok(child) => folder.children.push(Node::Folder(child)),
                Err(error) => issues.push(LoadIssue {
                    path: entry_path,
                    error,
                }),
            }
        } else if file_name == FOLDER_FILE || (is_root && file_name == COLLECTION_FILE) {
            match read_settings_file(&entry_path) {
                Ok(file) => {
                    if let Some((name, seq)) = folder_meta(&file) {
                        folder.name = name;
                        folder.seq = seq;
                    }
                    folder.vars = file.dict(FOLDER_VARS_BLOCK).unwrap_or_default().to_vec();
                }
                Err(error) => issues.push(LoadIssue {
                    path: entry_path,
                    error,
                }),
            }
        } else if file_name == COLLECTION_FILE || !file_name.ends_with(".bru") {
            continue;
        } else {
            match summarize_request(&entry_path) {
                Ok(summary) => folder.children.push(Node::Request(summary)),
                Err(error) => issues.push(LoadIssue {
                    path: entry_path,
                    error,
                }),
            }
        }
    }

    folder
        .children
        .sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    Ok(folder)
}

fn read_settings_file(path: &Path) -> Result<BruFile, CoreError> {
    let source = std::fs::read_to_string(path).map_err(|e| CoreError::io(path, e))?;
    bru::parse(&source).map_err(|e| CoreError::from(e).in_file(path))
}

fn folder_meta(file: &BruFile) -> Option<(String, Option<u32>)> {
    let meta = file.dict("meta")?;
    let get = |k: &str| meta.iter().find(|e| e.name == k).map(|e| e.value.as_str());
    Some((
        get("name")?.to_owned(),
        get("seq").and_then(|s| s.parse().ok()),
    ))
}

fn summarize_request(path: &Path) -> Result<RequestSummary, CoreError> {
    let request = load_request(path)?;
    let name = if request.meta.name.is_empty() {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        request.meta.name
    };
    Ok(RequestSummary {
        name,
        method: request.method,
        seq: request.meta.seq,
        path: path.to_path_buf(),
    })
}

fn load_environments(dir: &Path, issues: &mut Vec<LoadIssue>) -> Vec<Environment> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut envs: Vec<Environment> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "bru"))
        .filter_map(|p| match Environment::load(&p) {
            Ok(env) => Some(env),
            Err(error) => {
                issues.push(LoadIssue { path: p, error });
                None
            }
        })
        .collect();
    envs.sort_by(|a, b| a.name.cmp(&b.name));
    envs
}

// ---------------------------------------------------------------------------
// Scaffolding: creating collections, folders and requests on disk
// ---------------------------------------------------------------------------

/// Turn a display name into a portable file/folder name (Windows-safe).
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim_end_matches(['.', ' ']).to_owned();
    if cleaned.is_empty() {
        "untitled".to_owned()
    } else {
        cleaned
    }
}

/// `dir/stem.ext`, or `dir/stem-2.ext`, `dir/stem-3.ext`... if taken.
fn unique_path(dir: &Path, stem: &str, extension: Option<&str>) -> PathBuf {
    let make = |suffix: String| {
        let name = match extension {
            Some(ext) => format!("{stem}{suffix}.{ext}"),
            None => format!("{stem}{suffix}"),
        };
        dir.join(name)
    };
    let mut candidate = make(String::new());
    let mut n = 2;
    while candidate.exists() {
        candidate = make(format!("-{n}"));
        n += 1;
    }
    candidate
}

/// Create `parent/<name>/` with a `bruno.json` manifest. Returns its path.
pub fn create_collection(parent: &Path, name: &str) -> Result<PathBuf, CoreError> {
    let dir = unique_path(parent, &sanitize_file_name(name), None);
    std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
    write_manifest(&dir, name)?;
    Ok(dir)
}

/// Turn an existing folder into a collection by adding a `bruno.json`
/// (named after the folder) if it doesn't have one yet.
pub fn ensure_collection(dir: &Path) -> Result<(), CoreError> {
    if dir.join(COLLECTION_MANIFEST).exists() {
        return Ok(());
    }
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Collection".to_owned());
    write_manifest(dir, &name)
}

fn write_manifest(dir: &Path, name: &str) -> Result<(), CoreError> {
    let manifest = serde_json::json!({
        "version": "1",
        "name": name.trim(),
        "type": "collection",
        "ignore": ["node_modules", ".git"],
    });
    let path = dir.join(COLLECTION_MANIFEST);
    let text = serde_json::to_string_pretty(&manifest).unwrap_or_default() + "\n";
    std::fs::write(&path, text).map_err(|e| CoreError::io(&path, e))
}

/// Create `dir/<name>/folder.bru`. Returns the new folder's path.
pub fn create_folder(dir: &Path, name: &str) -> Result<PathBuf, CoreError> {
    let folder = unique_path(dir, &sanitize_file_name(name), None);
    std::fs::create_dir_all(&folder).map_err(|e| CoreError::io(&folder, e))?;
    let meta = BruFile {
        blocks: vec![Block::dict(
            "meta",
            vec![
                KeyValue::new("name", name.trim()),
                KeyValue::new("seq", next_seq(dir).to_string()),
            ],
        )],
    };
    let path = folder.join(FOLDER_FILE);
    std::fs::write(&path, bru::write(&meta)).map_err(|e| CoreError::io(&path, e))?;
    Ok(folder)
}

/// Create a new GET request file in `dir`, sequenced after its siblings.
pub fn create_request(dir: &Path, name: &str) -> Result<PathBuf, CoreError> {
    let path = unique_path(dir, &sanitize_file_name(name), Some("bru"));
    let mut request = HttpRequest::new(name.trim(), HttpMethod::Get, "");
    request.meta.seq = Some(next_seq(dir));
    std::fs::write(&path, bru::write_request(&request)).map_err(|e| CoreError::io(&path, e))?;
    Ok(path)
}

/// One more than the highest `seq` among the requests and folders in `dir`.
fn next_seq(dir: &Path) -> u32 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 1;
    };
    let seq_of = |path: &Path| -> Option<u32> {
        let source = std::fs::read_to_string(path).ok()?;
        let file = bru::parse(&source).ok()?;
        let meta = file.dict("meta")?;
        meta.iter()
            .find(|e| e.name == "seq")?
            .value
            .trim()
            .parse()
            .ok()
    };
    entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.is_dir() {
                seq_of(&path.join(FOLDER_FILE))
            } else if path.extension().is_some_and(|x| x == "bru")
                && path
                    .file_name()
                    .is_some_and(|n| n != COLLECTION_FILE && n != FOLDER_FILE)
            {
                seq_of(&path)
            } else {
                None
            }
        })
        .max()
        .map_or(1, |max| max + 1)
}
