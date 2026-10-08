//! Loading a Bruno collection directory into a lightweight tree.
//!
//! The tree only keeps [`RequestSummary`] values (name, method, seq, path):
//! full requests are parsed on demand when a tab is opened. This keeps memory
//! flat for large collections while still making the sidebar and command
//! palette instantaneous.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bru;
use crate::env::Environment;
use crate::error::CoreError;
use crate::model::{HttpMethod, HttpRequest};

pub const COLLECTION_MANIFEST: &str = "bruno.json";
pub const COLLECTION_FILE: &str = "collection.bru";
pub const FOLDER_FILE: &str = "folder.bru";
pub const ENVIRONMENTS_DIR: &str = "environments";

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
}

/// Read and fully parse a request file.
pub fn load_request(path: &Path) -> Result<HttpRequest, CoreError> {
    let source = std::fs::read_to_string(path).map_err(|e| CoreError::io(path, e))?;
    bru::parse_request(&source).map_err(|e| e.in_file(path))
}

/// Serialize and atomically write a request file (write + rename), so a
/// crash mid-save never leaves a truncated `.bru` behind.
pub fn save_request(path: &Path, request: &HttpRequest) -> Result<(), CoreError> {
    let tmp = path.with_extension("bru.tmp");
    std::fs::write(&tmp, bru::write_request(request)).map_err(|e| CoreError::io(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| CoreError::io(path, e))
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
        } else if file_name == FOLDER_FILE {
            if let Some((name, seq)) = read_folder_meta(&entry_path) {
                folder.name = name;
                folder.seq = seq;
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

fn read_folder_meta(path: &Path) -> Option<(String, Option<u32>)> {
    let source = std::fs::read_to_string(path).ok()?;
    let file = bru::parse(&source).ok()?;
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
