//! Synthetic files: `/proc`, `/sys`, `/dev` and `/etc` content the guest probes at start-up.

use crate::error::{Result, VfsError};
use crate::fs::{seek_in, DirEntry, FileKind, FileSystem, OpenFlags, Stat, VFile};
use crate::path::components;
use std::collections::{BTreeMap, BTreeSet};
use std::io::SeekFrom;
use std::sync::Arc;

/// Content of a synthetic node.
#[derive(Clone)]
pub enum Node {
    /// Fixed bytes.
    Bytes(Arc<[u8]>),
    /// `/dev/null`: reads give EOF, writes are swallowed.
    Null,
    /// `/dev/urandom`: infinite host randomness.
    Random,
    /// `/dev/zero`: endless zero bytes, writes are swallowed.
    Zero,
    /// A symbolic link (`/proc/self/exe`); only `readlink` follows it.
    Link(Arc<str>),
}

/// A read-mostly tree of synthetic files, built once and immutable afterwards.
#[derive(Default)]
pub struct MemFs {
    nodes: BTreeMap<String, Node>,
    dirs: BTreeSet<String>,
}

impl MemFs {
    /// An empty tree (the root exists).
    pub fn new() -> Self {
        let mut m = MemFs::default();
        m.dirs.insert("/".to_string());
        m
    }

    /// Adds a node at `path` (normalised, absolute), creating parent directories.
    pub fn insert(&mut self, path: &str, node: Node) {
        let mut cur = String::new();
        let parts: Vec<&str> = components(path).collect();
        for p in &parts[..parts.len() - 1] {
            cur.push('/');
            cur.push_str(p);
            self.dirs.insert(cur.clone());
        }
        self.nodes.insert(path.to_string(), node);
    }

    /// Adds a symbolic link at `path` pointing at `target`.
    pub fn insert_link(&mut self, path: &str, target: &str) {
        self.insert(path, Node::Link(target.into()));
    }

    /// Adds a text file.
    pub fn insert_text(&mut self, path: &str, text: &str) {
        self.insert(path, Node::Bytes(text.as_bytes().to_vec().into()));
    }
}

struct MemFile {
    node: Node,
    pos: u64,
}

impl VFile for MemFile {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        match &self.node {
            Node::Null | Node::Link(_) => Ok(0),
            Node::Zero => {
                buf.fill(0);
                Ok(buf.len())
            }
            Node::Random => {
                getrandom::fill(buf).map_err(|_| VfsError::Io)?;
                Ok(buf.len())
            }
            Node::Bytes(b) => {
                let start = (self.pos as usize).min(b.len());
                let n = buf.len().min(b.len() - start);
                buf[..n].copy_from_slice(&b[start..start + n]);
                self.pos = (start + n) as u64;
                Ok(n)
            }
        }
    }

    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        match self.node {
            Node::Null | Node::Random | Node::Zero => Ok(buf.len()),
            Node::Bytes(_) | Node::Link(_) => Err(VfsError::BadFd),
        }
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.pos = seek_in(self.len()?, self.pos, pos)?;
        Ok(self.pos)
    }

    fn len(&mut self) -> Result<u64> {
        Ok(match &self.node {
            Node::Bytes(b) => b.len() as u64,
            _ => 0,
        })
    }

    fn set_len(&mut self, _len: u64) -> Result<()> {
        Err(VfsError::BadFd)
    }

    fn sync(&mut self) -> Result<()> {
        Ok(())
    }
}

impl FileSystem for MemFs {
    fn open(&self, path: &str, flags: OpenFlags) -> Result<Box<dyn VFile>> {
        match self.nodes.get(path) {
            Some(node) => {
                if matches!(node, Node::Link(_)) {
                    // The target lives in another backend; only `readlink` follows the link.
                    return Err(VfsError::NotFound);
                }
                let device = !matches!(node, Node::Bytes(_));
                if (flags.write || flags.truncate || flags.create) && !device {
                    return Err(VfsError::ReadOnly);
                }
                if flags.directory {
                    return Err(VfsError::NotDir);
                }
                Ok(Box::new(MemFile { node: node.clone(), pos: 0 }))
            }
            None if self.dirs.contains(path) => Err(VfsError::IsDir),
            None => Err(VfsError::NotFound),
        }
    }

    fn stat(&self, path: &str) -> Result<Stat> {
        match self.nodes.get(path) {
            Some(Node::Bytes(b)) => Ok(Stat::file(b.len() as u64)),
            Some(Node::Link(_)) => Ok(Stat::file(0)),
            Some(_) => Ok(Stat { readonly: false, ..Stat::file(0) }),
            None if self.dirs.contains(path) => Ok(Stat::dir()),
            None => Err(VfsError::NotFound),
        }
    }

    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        if self.nodes.contains_key(path) {
            return Err(VfsError::NotDir);
        }
        if !self.dirs.contains(path) {
            return Err(VfsError::NotFound);
        }
        let prefix = if path == "/" { "/".to_string() } else { format!("{path}/") };
        let mut out: BTreeMap<String, FileKind> = BTreeMap::new();
        for (p, kind) in
            self.nodes.keys().map(|p| (p, FileKind::File)).chain(self.dirs.iter().map(|p| (p, FileKind::Dir)))
        {
            if let Some(rest) = p.strip_prefix(&prefix) {
                if !rest.is_empty() && !rest.contains('/') {
                    out.insert(rest.to_string(), kind);
                }
            }
        }
        Ok(out.into_iter().map(|(name, kind)| DirEntry { name, kind }).collect())
    }

    fn readlink(&self, path: &str) -> Result<String> {
        match self.nodes.get(path) {
            Some(Node::Link(t)) => Ok(t.to_string()),
            Some(_) => Err(VfsError::Invalid),
            None if self.dirs.contains(path) => Err(VfsError::Invalid),
            None => Err(VfsError::NotFound),
        }
    }
}
