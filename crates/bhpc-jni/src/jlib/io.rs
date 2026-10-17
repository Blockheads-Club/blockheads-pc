//! `java.io`.

use super::{boolean, int, long, object, string, void};
use crate::value::{Obj, Value};
use crate::vm::{Call, Data, JResult, Vm};
use std::path::PathBuf;

fn path_of(c: &Call, o: Obj) -> String {
    c.vm.get_field(o, "path").as_ref().and_then(|s| c.vm.string(s)).unwrap_or_default()
}

fn this_path(c: &Call) -> String {
    path_of(c, c.this)
}

/// A new `java.io.File` for `path`.
pub(crate) fn new_file(vm: &Vm, path: &str) -> Obj {
    let k = vm.find_class("java/io/File").unwrap_or(crate::value::ClassId(0));
    let f = vm.alloc_object(k);
    let s = vm.new_string(path);
    vm.set_field(f, "path", Value::Ref(Some(s)));
    f
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", dir.trim_end_matches('/'), name)
    }
}

/// A new in-memory stream over `data`.
pub(crate) fn new_stream(vm: &Vm, data: Vec<u8>) -> Obj {
    vm.new_data_object("java/io/ByteArrayInputStream", Data::Bytes { data, pos: 0 })
}

fn with_stream<R>(c: &Call, f: impl FnOnce(&mut Vec<u8>, &mut usize) -> R) -> Option<R> {
    c.vm.with_data(c.this, |d| match d {
        Data::Bytes { data, pos } => Some(f(data, pos)),
        _ => None,
    })
    .flatten()
}

/// `read(byte[], off, len)`.
fn read_into(c: &mut Call, arr: Option<Obj>, off: i32, len: i32) -> JResult {
    let Some(arr) = arr else { return Err(c.vm.throw_new("java/lang/NullPointerException", "buffer")) };
    let n = c.vm.array_len(arr).unwrap_or(0);
    if off < 0 || len < 0 || off as usize + len as usize > n {
        return Err(c.vm.throw_new("java/lang/IndexOutOfBoundsException", "read"));
    }
    if len == 0 {
        return int(0);
    }
    let chunk = with_stream(c, |d, p| {
        let end = (*p + len as usize).min(d.len());
        let out = d[*p..end].to_vec();
        *p = end;
        out
    })
    .unwrap_or_default();
    if chunk.is_empty() {
        return int(-1);
    }
    c.vm.with_data(arr, |d| {
        if let Data::Prim(_, b) = d {
            b[off as usize..off as usize + chunk.len()].copy_from_slice(&chunk);
        }
    });
    int(chunk.len() as i32)
}

pub(crate) fn install(vm: &Vm) {
    vm.define("java/io/File")
        .field("path", "Ljava/lang/String;")
        .method("<init>", "(Ljava/lang/String;)V", |c| {
            c.vm.set_field(c.this, "path", c.args[0]);
            void()
        })
        .method("<init>", "(Ljava/lang/String;Ljava/lang/String;)V", |c| {
            let p = join(&c.string(0).unwrap_or_default(), &c.string(1).unwrap_or_default());
            let s = c.vm.new_string(&p);
            c.vm.set_field(c.this, "path", Value::Ref(Some(s)));
            void()
        })
        .method("<init>", "(Ljava/io/File;Ljava/lang/String;)V", |c| {
            let dir = c.obj(0).map(|d| path_of(c, d)).unwrap_or_default();
            let s = c.vm.new_string(&join(&dir, &c.string(1).unwrap_or_default()));
            c.vm.set_field(c.this, "path", Value::Ref(Some(s)));
            void()
        })
        .method("getPath", "()Ljava/lang/String;", |c| Ok(c.vm.get_field(c.this, "path")))
        .method("toString", "()Ljava/lang/String;", |c| Ok(c.vm.get_field(c.this, "path")))
        .method("getAbsolutePath", "()Ljava/lang/String;", |c| Ok(c.vm.get_field(c.this, "path")))
        .method("getName", "()Ljava/lang/String;", |c| {
            let p = this_path(c);
            string(c, p.trim_end_matches('/').rsplit('/').next().unwrap_or_default())
        })
        .method("getParent", "()Ljava/lang/String;", |c| {
            let p = this_path(c);
            match p.trim_end_matches('/').rsplit_once('/') {
                Some((d, _)) => string(c, d),
                None => object(None),
            }
        })
        .method("getParentFile", "()Ljava/io/File;", |c| {
            let p = this_path(c);
            let d = p.trim_end_matches('/').rsplit_once('/').map(|(d, _)| d.to_string());
            object(d.map(|d| new_file(c.vm, &d)))
        })
        .method("exists", "()Z", |c| boolean(PathBuf::from(this_path(c)).exists()))
        .method("isDirectory", "()Z", |c| boolean(PathBuf::from(this_path(c)).is_dir()))
        .method("isFile", "()Z", |c| boolean(PathBuf::from(this_path(c)).is_file()))
        .method("canRead", "()Z", |c| boolean(PathBuf::from(this_path(c)).exists()))
        .method("canWrite", "()Z", |c| boolean(PathBuf::from(this_path(c)).exists()))
        .method("length", "()J", |c| long(std::fs::metadata(this_path(c)).map_or(0, |m| m.len() as i64)))
        .method("mkdirs", "()Z", |c| boolean(std::fs::create_dir_all(this_path(c)).is_ok()))
        .method("mkdir", "()Z", |c| boolean(std::fs::create_dir(this_path(c)).is_ok()))
        .method("delete", "()Z", |c| {
            let p = this_path(c);
            boolean(std::fs::remove_file(&p).or_else(|_| std::fs::remove_dir(&p)).is_ok())
        })
        .method("list", "()[Ljava/lang/String;", |c| match std::fs::read_dir(this_path(c)) {
            Ok(rd) => {
                let mut names: Vec<String> =
                    rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
                names.sort();
                object(Some(c.vm.new_string_array(&names)))
            }
            Err(_) => object(None),
        });

    vm.define("java/io/InputStream").implements(&["java/io/Closeable"]);
    vm.define("java/io/Closeable").interface();
    vm.define("java/io/ByteArrayInputStream")
        .extends("java/io/InputStream")
        .payload(|| Data::Bytes { data: Vec::new(), pos: 0 })
        .method("<init>", "([B)V", |c| {
            let bytes = match c.obj(0).and_then(|o| c.vm.data(o)) {
                Some(Data::Prim(_, b)) => b,
                _ => Vec::new(),
            };
            c.vm.with_data(c.this, |d| *d = Data::Bytes { data: bytes, pos: 0 });
            void()
        })
        .method("read", "()I", |c| {
            int(with_stream(c, |d, p| {
                let b = d.get(*p).map_or(-1, |b| *b as i32);
                *p += (b >= 0) as usize;
                b
            })
            .unwrap_or(-1))
        })
        .method("read", "([B)I", |c| {
            let n = c.obj(0).and_then(|a| c.vm.array_len(a)).unwrap_or(0) as i32;
            read_into(c, c.obj(0), 0, n)
        })
        .method("read", "([BII)I", |c| read_into(c, c.obj(0), c.int(1), c.int(2)))
        .method("available", "()I", |c| int(with_stream(c, |d, p| d.len() - *p).unwrap_or(0) as i32))
        .method("skip", "(J)J", |c| {
            let n = c.long(0).max(0) as usize;
            long(
                with_stream(c, |d, p| {
                    let step = n.min(d.len() - *p);
                    *p += step;
                    step
                })
                .unwrap_or(0) as i64,
            )
        })
        .method("close", "()V", |_| void());
    vm.define("java/io/FileInputStream").extends("java/io/ByteArrayInputStream").method(
        "<init>",
        "(Ljava/lang/String;)V",
        |c| open_file(c, c.string(0).unwrap_or_default()),
    );
    vm.define("java/io/FileInputStream").method("<init>", "(Ljava/io/File;)V", |c| {
        let p = c.obj(0).map(|f| path_of(c, f)).unwrap_or_default();
        open_file(c, p)
    });
}

fn open_file(c: &mut Call, path: String) -> JResult {
    match std::fs::read(&path) {
        Ok(data) => {
            c.vm.with_data(c.this, |d| *d = Data::Bytes { data, pos: 0 });
            void()
        }
        Err(e) => Err(c.vm.throw_new("java/io/FileNotFoundException", &format!("{path}: {e}"))),
    }
}
