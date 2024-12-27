use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{Read, Seek},
    path::{Component, Path, PathBuf},
};

use regex::Regex;

pub mod archives;
pub mod network;
pub mod recipe;
pub mod source;

pub trait ESPIN {
    fn extract_stripping_prefix_if_needed<P: AsRef<Path>>(&mut self, directory: P, in_prefix: P);
    fn make_writable_dir_all<T: AsRef<Path>>(outpath: T);
}
impl<R: Read + Seek> ESPIN for zip::ZipArchive<R> {
    fn extract_stripping_prefix_if_needed<P: AsRef<Path>>(&mut self, directory: P, in_prefix: P) {
        let file_names: Vec<_> = self.file_names().map(|x| x.to_owned()).collect();
        let mut try_prefix_dir = PathBuf::from(&file_names[0])
            .components()
            .filter_map(|x| {
                if let Component::Normal(v) = x {
                    Some(v)
                } else {
                    None
                }
            })
            .next()
            .map(|x| x.to_owned());
        for i in &file_names {
            if let Some(v) = &try_prefix_dir {
                if PathBuf::from(&i)
                    .components()
                    .filter_map(|x| {
                        if let Component::Normal(v) = x {
                            Some(v)
                        } else {
                            None
                        }
                    })
                    .next()
                    .map(|x| x.to_owned())
                    != Some(v.to_owned())
                {
                    try_prefix_dir = None;
                }
            } else {
                break;
            }
        }
        let mut files_by_unix_mode = Vec::new();
        for i in 0..self.len() {
            let mut file = self.by_index(i).unwrap();
            let filepath = file.enclosed_name().unwrap();

            let filepath = if let Some(v) = &try_prefix_dir {
                filepath
                    .strip_prefix(v)
                    .unwrap()
                    .strip_prefix(&in_prefix)
                    .unwrap()
            } else {
                filepath.strip_prefix(&in_prefix).unwrap()
            };

            let outpath = directory.as_ref().join(filepath);

            if file.is_dir() {
                Self::make_writable_dir_all(&outpath);
                continue;
            }
            let symlink_target = if file.is_symlink() && (cfg!(unix) || cfg!(windows)) {
                let mut target = Vec::with_capacity(file.size() as usize);
                file.read_to_end(&mut target).unwrap();
                Some(target)
            } else {
                None
            };
            drop(file);
            if let Some(p) = outpath.parent() {
                Self::make_writable_dir_all(p);
            }
            if let Some(target) = symlink_target {
                use std::os::unix::ffi::OsStringExt;
                let target = OsString::from_vec(target);
                std::os::unix::fs::symlink(&target, outpath.as_path()).unwrap();
                continue;
            }
            let mut file = self.by_index(i).unwrap();
            let mut outfile = std::fs::File::create(&outpath).unwrap();
            std::io::copy(&mut file, &mut outfile).unwrap();
            // Check for real permissions, which we'll set in a second pass
            if let Some(mode) = file.unix_mode() {
                files_by_unix_mode.push((outpath.clone(), mode));
            }
        }
        use std::cmp::Reverse;
        use std::os::unix::fs::PermissionsExt;

        if files_by_unix_mode.len() > 1 {
            // Ensure we update children's permissions before making a parent unwritable
            files_by_unix_mode.sort_by_key(|(path, _)| Reverse(path.clone()));
        }
        for (path, mode) in files_by_unix_mode.into_iter() {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
    }

    fn make_writable_dir_all<T: AsRef<Path>>(outpath: T) {
        std::fs::create_dir_all(outpath.as_ref()).unwrap();

        // Dirs must be writable until all normal files are extracted
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            outpath.as_ref(),
            std::fs::Permissions::from_mode(
                0o700
                    | std::fs::metadata(outpath.as_ref())
                        .unwrap()
                        .permissions()
                        .mode(),
            ),
        )
        .unwrap();
    }
}

pub fn sanitize_path(p: &Path) -> PathBuf {
    let mut p = path_clean::clean(p);
    if p.is_absolute() {
        p = p.strip_prefix("/").unwrap().to_owned();
    }
    assert!(!p.starts_with(".."));
    if p == PathBuf::from(".") {
        PathBuf::new()
    } else {
        p
    }
}

pub fn env_substitute(line: &str, env: &BTreeMap<String, String>) -> String {
    let mut line = line.to_owned();
    loop {
        let mut changed = false;
        let simple_re = Regex::new(r"(^|[^\\])\$([a-zA-Z_][a-zA-Z_0-9]*)").unwrap();
        let brace_re = Regex::new(r"(^|[^\\])\$\{([a-zA-Z_][a-zA-Z_0-9]*)\}").unwrap();
        line = simple_re
            .replace_all(&line, |captures: &regex::Captures<'_>| {
                changed = true;
                captures.get(1).unwrap().as_str().to_owned()
                    + env.get(captures.get(2).unwrap().as_str()).expect(&format!(
                        "no env var found: {}",
                        captures.get(2).unwrap().as_str()
                    ))
            })
            .to_string();
        line = brace_re
            .replace_all(&line, |captures: &regex::Captures<'_>| {
                changed = true;
                captures.get(1).unwrap().as_str().to_owned()
                    + env.get(captures.get(2).unwrap().as_str()).unwrap()
            })
            .to_string();
        if !changed {
            return line;
        }
    }
}

#[repr(u8)]
#[derive(Debug, PartialEq)]
pub enum WorkerStatus {
    ReadyForWork,
    ReadyForSource,
    ReadyForOverlay,
    HaveSource,
    NeedSource,
    ReadyForDep,
    HaveDep,
    NeedDep,
    HaveOverlay,
    NeedOverlay,
    ReadyForEnvs,
    BuildComplete,
}

#[test]
fn test_sanitize_path_empty() {
    assert_eq!(sanitize_path(&PathBuf::new()), PathBuf::new())
}

#[test]
fn test_sanitize_path_dot() {
    assert_eq!(sanitize_path(&PathBuf::from(".")), PathBuf::new())
}

#[test]
#[should_panic]
fn test_sanitize_path_dotdot() {
    assert_eq!(sanitize_path(&PathBuf::from("..")), PathBuf::new())
}
