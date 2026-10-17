//! Errors of the VFS, each mapping to a bionic (Linux ARM) `errno`.

use std::fmt;
use std::io;

/// Result alias of the whole crate.
pub type Result<T> = std::result::Result<T, VfsError>;

/// A VFS failure. The variants are the errors the guest can observe; [`VfsError::errno`] gives
/// the bionic value to store in the guest `errno`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsError {
    /// `ENOENT`
    NotFound,
    /// `EIO`: host I/O failure or corrupt archive entry.
    Io,
    /// `EBADF`: write on a read-only descriptor and the like.
    BadFd,
    /// `EACCES`
    PermissionDenied,
    /// `EEXIST`
    Exists,
    /// `EXDEV`: rename across mounts.
    CrossDevice,
    /// `ENOTDIR`
    NotDir,
    /// `EISDIR`
    IsDir,
    /// `EINVAL`
    Invalid,
    /// `EROFS`
    ReadOnly,
    /// `ENOTEMPTY`
    NotEmpty,
}

impl VfsError {
    /// The bionic `errno` value.
    pub fn errno(self) -> i32 {
        match self {
            VfsError::NotFound => 2,
            VfsError::Io => 5,
            VfsError::BadFd => 9,
            VfsError::PermissionDenied => 13,
            VfsError::Exists => 17,
            VfsError::CrossDevice => 18,
            VfsError::NotDir => 20,
            VfsError::IsDir => 21,
            VfsError::Invalid => 22,
            VfsError::ReadOnly => 30,
            VfsError::NotEmpty => 39,
        }
    }
}

impl fmt::Display for VfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            VfsError::NotFound => "no such file or directory",
            VfsError::Io => "input/output error",
            VfsError::BadFd => "bad file descriptor",
            VfsError::PermissionDenied => "permission denied",
            VfsError::Exists => "file exists",
            VfsError::CrossDevice => "cross-device link",
            VfsError::NotDir => "not a directory",
            VfsError::IsDir => "is a directory",
            VfsError::Invalid => "invalid argument",
            VfsError::ReadOnly => "read-only file system",
            VfsError::NotEmpty => "directory not empty",
        };
        f.write_str(s)
    }
}

impl std::error::Error for VfsError {}

impl From<io::Error> for VfsError {
    fn from(e: io::Error) -> Self {
        use io::ErrorKind as K;
        match e.kind() {
            K::NotFound => VfsError::NotFound,
            K::PermissionDenied => VfsError::PermissionDenied,
            K::AlreadyExists => VfsError::Exists,
            K::NotADirectory => VfsError::NotDir,
            K::IsADirectory => VfsError::IsDir,
            K::DirectoryNotEmpty => VfsError::NotEmpty,
            K::ReadOnlyFilesystem => VfsError::ReadOnly,
            K::InvalidInput => VfsError::Invalid,
            _ => VfsError::Io,
        }
    }
}

impl From<zip::result::ZipError> for VfsError {
    fn from(e: zip::result::ZipError) -> Self {
        match e {
            zip::result::ZipError::FileNotFound => VfsError::NotFound,
            _ => VfsError::Io,
        }
    }
}
