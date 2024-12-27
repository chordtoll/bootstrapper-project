use std::{
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use zip::ZipArchive;

use crate::sanitize_path;

pub trait Archiver {
    fn filter_extract(
        &mut self,
        target: &Path,
        filter: impl Fn(&Path, &Option<PathBuf>) -> Option<PathBuf>,
    );
}

impl<R: std::io::Read + std::io::Seek> Archiver for tar::Archive<R> {
    fn filter_extract(
        &mut self,
        target: &Path,
        filter: impl Fn(&Path, &Option<PathBuf>) -> Option<PathBuf>,
    ) {
        let mut itr = self.entries().unwrap();
        let mut prefix = Some(
            PathBuf::from(itr.next().unwrap().unwrap().path().unwrap())
                .ancestors()
                .next()
                .unwrap()
                .to_owned(),
        );

        for i in itr {
            if Some(
                PathBuf::from(i.unwrap().path().unwrap())
                    .ancestors()
                    .next()
                    .unwrap()
                    .to_owned(),
            ) != prefix
            {
                prefix = None;
                break;
            }
        }
        self.reset();
        for i in self.entries().unwrap() {
            let mut i = i.unwrap();
            let target_path = filter(&i.path().unwrap(), &prefix);
            if let Some(path) = target_path {
                let target_path = target.join(sanitize_path(&path));
                std::fs::create_dir_all(target_path.parent().unwrap()).unwrap();
                i.unpack(target_path).unwrap();
            }
        }
    }
}

impl<R: std::io::Read + std::io::Seek> Archiver for ZipArchive<R> {
    fn filter_extract(
        &mut self,
        target: &Path,
        filter: impl Fn(&Path, &Option<PathBuf>) -> Option<PathBuf>,
    ) {
        let mut prefix = Some(
            PathBuf::from(self.by_index(0).unwrap().name())
                .ancestors()
                .next()
                .unwrap()
                .to_owned(),
        );
        for _ in 1..self.len() {
            if Some(
                PathBuf::from(self.by_index(0).unwrap().name())
                    .ancestors()
                    .next()
                    .unwrap()
                    .to_owned(),
            ) != prefix
            {
                prefix = None;
                break;
            }
        }
        for i in 0..self.len() {
            let mut i = self.by_index(i).unwrap();
            let target_path = filter(&PathBuf::from(i.name()), &prefix);
            if let Some(path) = target_path {
                let target_path = target.join(sanitize_path(&path));
                if i.is_dir() {
                    std::fs::create_dir_all(&target_path).unwrap();
                }
                if i.is_file() {
                    std::fs::create_dir_all(target_path.parent().unwrap()).unwrap();
                    let mut outfile = std::fs::File::create(&target_path).unwrap();
                    std::io::copy(&mut i, &mut outfile).unwrap();
                }
                if i.is_symlink() {
                    let mut sl = String::new();
                    i.read_to_string(&mut sl).unwrap();
                    if sl.starts_with("..") {
                        std::os::unix::fs::symlink(sl, &target_path).unwrap();
                    } else if !sl.starts_with("/") {
                        std::os::unix::fs::symlink(sl, &target_path).unwrap();
                    } else {
                        todo!("{:?}", sl)
                    }
                } else {
                    if let Some(mode) = i.unix_mode() {
                        std::fs::set_permissions(
                            target_path,
                            std::fs::Permissions::from_mode(mode),
                        )
                        .unwrap();
                    }
                }
            }
        }
    }
}

pub fn source_extract_filter<'a>(
    from: &'a Path,
    to: &'a Path,
    include: &'a Option<Vec<String>>,
    clean_root: bool,
) -> impl Fn(&Path, &Option<PathBuf>) -> Option<PathBuf> + 'a {
    move |p: &Path, prefix: &Option<PathBuf>| {
        print!(
            "{:?} {:?} {:?} {:?} {:?} {} ->",
            p, prefix, from, to, include, clean_root
        );

        let p = if clean_root {
            if let Some(prefix) = prefix {
                p.strip_prefix(prefix).unwrap()
            } else {
                p
            }
        } else {
            p
        };

        if let Some(include) = include {
            if !include
                .iter()
                .any(|x| p.starts_with(sanitize_path(&PathBuf::from(x))))
            {
                println!("None");
                return None;
            }
        }

        let p = sanitize_path(p);
        let p = p.strip_prefix(sanitize_path(from)).unwrap();
        let p = sanitize_path(p);
        let p = sanitize_path(to).join(p);
        let p = sanitize_path(&p);
        println!(" {:?}", p);
        Some(p)
    }
}
