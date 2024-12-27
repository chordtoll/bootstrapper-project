use std::{
    collections::BTreeMap,
    fs::{create_dir, create_dir_all, read_dir},
    io::Cursor,
    net::TcpStream,
    path::PathBuf,
    process::{Child, Command},
};

use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;
use bootstrapper::{
    archives::{source_extract_filter, Archiver},
    env_substitute,
    network::{read_deps, read_envs, read_overlays, read_recipe, read_sources, write_archive},
    recipe::{NamedRecipeVersion, RecipeBuildStep, SourceContents},
    sanitize_path, WorkerStatus,
};
use byteorder::{ReadBytesExt, WriteBytesExt};
use bzip2::read::BzDecoder;
use nix::{
    mount::{mount, umount, MsFlags},
    sys::stat::{makedev, mknod, Mode, SFlag},
};
use regex::Regex;
use tempfile::TempDir;

#[derive(Debug)]
enum StatusUpdate {
    CommandRun(Vec<String>),
    CommandOut(String),
    CommandError(String),
    CommandDone(i32),
    Done,
}

fn main() {
    let mut stream = TcpStream::connect("127.0.0.1:1234").unwrap();

    loop {
        stream.write_u8(WorkerStatus::ReadyForWork as u8).unwrap();

        if stream.read_u8().unwrap() == 1 { break; }

        let recipe = read_recipe(&mut stream);

        let source_data = read_sources(&mut stream);

        let dep_data = read_deps(&mut stream);

        let overlay_data = read_overlays(&mut stream);

        let env_data = read_envs(&mut stream);

        let (pq_s, pq_r) = std::sync::mpsc::channel();

        let jh = std::thread::spawn(|| {
            build(recipe, source_data, dep_data, overlay_data, env_data, pq_s)
        });

        while let Ok(msg) = pq_r.recv() {
            println!("{:?}", msg)
        }

        let (hash, archive) = jh.join().unwrap();

        write_archive(&mut stream, &hash, &archive);
        println!("{}", hash)
    }
}

fn do_step(
    step: &RecipeBuildStep,
    env_data: &mut BTreeMap<String, String>,
    cur_dir: &mut PathBuf,
    work_dir: &TempDir,
    status_updates: &std::sync::mpsc::Sender<StatusUpdate>,
    join_handles: &mut Vec<Child>,
) {
    let i;
    let must_be_serial;
    let bash_noescape;
    match step {
        RecipeBuildStep::Simple(cmd) => {
            i = cmd;
            must_be_serial = true;
            bash_noescape = false;
        }
        RecipeBuildStep::Complex { cmd, serial, bash } => {
            i = cmd;
            must_be_serial = *serial;
            bash_noescape = *bash;
        }
    }
    if i.split(' ').next().unwrap().contains('=') && !bash_noescape {
        let (k, v) = i.split_once('=').unwrap();
        env_data.insert(k.to_owned(), env_substitute(v, &env_data));
    } else {
        let cmd = if bash_noescape {
            ["bash", "-c", i].map(|x| x.to_owned()).to_vec()
        } else {
            let env_substitute = env_substitute(&i, &env_data);
            let i = env_substitute;
            let cmd = shlex::split(&i).unwrap_or_else(|| panic!("Failed at line: {}", i));
            if cmd[0] == "cd" {
                *cur_dir = sanitize_path(&cur_dir.join(cmd[1].clone()));
                return;
            }
            if cmd[0] == "alias" {
                todo!();
            }
            cmd
        };
        let builddir = URL_SAFE.encode(work_dir.path().as_os_str().as_encoded_bytes());
        let chdir = URL_SAFE.encode(cur_dir.as_os_str().as_encoded_bytes());
        let buildstep = URL_SAFE.encode(serde_yaml::to_string(&cmd).unwrap());
        let env = URL_SAFE.encode(serde_yaml::to_string(&env_data).unwrap());
        if must_be_serial {
            join_handles
                .drain(..)
                .for_each(|mut res: std::process::Child| assert!(res.wait().unwrap().success()));
        }
        status_updates.send(StatusUpdate::CommandRun(cmd)).unwrap();
        let mut res = Command::new("bootstrapper/target/debug/chroothelper")
            .arg(builddir)
            .arg(chdir)
            .arg(buildstep)
            .arg(env)
            .spawn()
            .unwrap();
        if must_be_serial {
            if !(res.wait().unwrap().success()) {
                std::process::abort()
            }
        } else {
            join_handles.push(res);
        }
    }
}

fn build(
    recipe: NamedRecipeVersion,
    source_data: BTreeMap<String, (SourceContents, Vec<u8>)>,
    dep_data: BTreeMap<String, Vec<u8>>,
    overlay_data: BTreeMap<PathBuf, Vec<u8>>,
    mut env_data: BTreeMap<String, String>,
    status_updates: std::sync::mpsc::Sender<StatusUpdate>,
) -> (String, Vec<u8>) {
    let work_dir = tempfile::tempdir_in("ramdir/").unwrap();
    create_dir(work_dir.path().join("dev")).unwrap();
    mknod(
        &work_dir.path().join("dev/null"),
        SFlag::S_IFCHR,
        Mode::S_IRUSR
            | Mode::S_IRGRP
            | Mode::S_IROTH
            | Mode::S_IWUSR
            | Mode::S_IWGRP
            | Mode::S_IWOTH,
        makedev(1, 3),
    )
    .unwrap();
    mknod(
        &work_dir.path().join("dev/zero"),
        SFlag::S_IFCHR,
        Mode::S_IRUSR
            | Mode::S_IRGRP
            | Mode::S_IROTH
            | Mode::S_IWUSR
            | Mode::S_IWGRP
            | Mode::S_IWOTH,
        makedev(1, 5),
    )
    .unwrap();
    mknod(
        &work_dir.path().join("dev/random"),
        SFlag::S_IFCHR,
        Mode::S_IRUSR
            | Mode::S_IRGRP
            | Mode::S_IROTH
            | Mode::S_IWUSR
            | Mode::S_IWGRP
            | Mode::S_IWOTH,
        makedev(1, 8),
    )
    .unwrap();
    mknod(
        &work_dir.path().join("dev/urandom"),
        SFlag::S_IFCHR,
        Mode::S_IRUSR
            | Mode::S_IRGRP
            | Mode::S_IROTH
            | Mode::S_IWUSR
            | Mode::S_IWGRP
            | Mode::S_IWOTH,
        makedev(1, 9),
    )
    .unwrap();
    mknod(
        &work_dir.path().join("dev/ptmx"),
        SFlag::S_IFCHR,
        Mode::S_IRUSR
            | Mode::S_IRGRP
            | Mode::S_IROTH
            | Mode::S_IWUSR
            | Mode::S_IWGRP
            | Mode::S_IWOTH,
        makedev(5, 2),
    )
    .unwrap();
    create_dir(work_dir.path().join("dev/pts")).unwrap();
    mount(
        Some("/dev/pts"),
        &work_dir.path().join("dev/pts"),
        Some("devpts"),
        MsFlags::empty(),
        None::<&[u8]>,
    )
    .unwrap();
    create_dir(work_dir.path().join("proc")).unwrap();
    mount(
        Some("/proc"),
        &work_dir.path().join("proc"),
        Some("proc"),
        MsFlags::empty(),
        None::<&[u8]>,
    )
    .unwrap();
    let mut cur_dir = PathBuf::from("/");
    if let Some(sources) = recipe.source {
        for (name, source_directive) in sources {
            let (source, data) = source_data
                .get(&name)
                .expect(&format!("Missing source {}", name));
            assert!(source_directive.chmod.is_none());
            if let Some(extract) = source_directive.extract {
                if source.url.ends_with(".zip") {
                    zip::ZipArchive::new(std::io::Cursor::new(data))
                        .unwrap()
                        .filter_extract(
                            work_dir.path(),
                            source_extract_filter(
                                &PathBuf::new(),
                                &PathBuf::from(extract),
                                &source_directive.copy,
                                true,
                            ),
                        )
                } else {
                    todo!("{}", source.url);
                }
            }
            if let Some(noextract) = source_directive.noextract {
                assert!(source_directive.copy.is_none());
                let path = work_dir
                    .path()
                    .join(sanitize_path(&PathBuf::from(noextract)));
                std::fs::create_dir_all(&path.parent().unwrap()).unwrap();
                std::fs::write(path, data).unwrap();
            }
        }
    }
    if let Some(deps) = recipe.deps {
        for dep in deps {
            let data = dep_data
                .get(&format!("{}:{}", dep.name, dep.version))
                .expect(&format!("Missing dep {:?}", dep));
            tar::Archive::new(std::io::Cursor::new(data)).filter_extract(
                work_dir.path(),
                source_extract_filter(
                    &dep.from.map(|x| PathBuf::from(x)).unwrap_or(PathBuf::new()),
                    &dep.to.map(|x| PathBuf::from(x)).unwrap_or(PathBuf::new()),
                    &None,
                    false,
                ),
            )
        }
    }
    if let Some(_shell) = recipe.shell {
        todo!();
    }

    for (path, contents) in overlay_data {
        let target_path = work_dir.path().join(path);
        std::fs::create_dir_all(target_path.parent().unwrap()).unwrap();
        std::fs::write(target_path, contents).unwrap();
    }

    if let Some(mkdirs) = recipe.mkdirs {
        for mkdir in mkdirs {
            std::fs::create_dir_all(work_dir.path().join(sanitize_path(&PathBuf::from(mkdir))))
                .unwrap();
        }
    }
    let mut join_handles = Vec::new();
    match recipe.build {
        bootstrapper::recipe::RecipeBuildSteps::Single { single } => {
            for step in single {
                do_step(
                    &step,
                    &mut env_data,
                    &mut cur_dir,
                    &work_dir,
                    &status_updates,
                    &mut join_handles,
                );
            }
        }
        bootstrapper::recipe::RecipeBuildSteps::Piecewise {
            unpack,
            unpack_dirname,
            patch_dir,
            package_dir,
            prepare,
            configure,
            compile,
            install,
            postprocess,
        } => {
            let (pkg, pass) = if let Some(v) = Regex::new(r"^(.*)-pass([0-9]+)")
                .unwrap()
                .captures(&recipe.version)
            {
                let pass: u32 = v.get(2).unwrap().as_str().parse().unwrap();
                (
                    format!(
                        "{}-{}",
                        recipe.name.split('/').last().unwrap(),
                        v.get(1).unwrap().as_str()
                    ),
                    pass - 1,
                )
            } else {
                (
                    format!(
                        "{}-{}",
                        recipe.name.split('/').last().unwrap(),
                        recipe.version
                    ),
                    0,
                )
            };
            let pkg = if let Some(package_dir) = package_dir {
                package_dir.to_owned()
            } else {
                pkg
            };
            env_data.insert("pkg".to_owned(), pkg.clone());
            cur_dir = PathBuf::from("/steps/").join(&pkg);
            env_data.insert(
                "base_dir".to_owned(),
                PathBuf::from("/steps/")
                    .join(&pkg)
                    .as_os_str()
                    .to_str()
                    .unwrap()
                    .to_owned(),
            );
            env_data.insert(
                "patch_dir".to_owned(),
                PathBuf::from("/steps/")
                    .join(&pkg)
                    .join(patch_dir)
                    .as_os_str()
                    .to_str()
                    .unwrap()
                    .to_owned(),
            );
            env_data.insert(
                "mk_dir".to_owned(),
                PathBuf::from("/steps/")
                    .join(&pkg)
                    .join("mk")
                    .as_os_str()
                    .to_str()
                    .unwrap()
                    .to_owned(),
            );
            env_data.insert(
                "files_dir".to_owned(),
                PathBuf::from("/steps/")
                    .join(&pkg)
                    .join("files")
                    .as_os_str()
                    .to_str()
                    .unwrap()
                    .to_owned(),
            );
            env_data.insert("revision".to_owned(), pass.to_string());
            do_step(
                &RecipeBuildStep::Simple("mkdir build".to_owned()),
                &mut env_data,
                &mut cur_dir,
                &work_dir,
                &status_updates,
                &mut join_handles,
            );
            cur_dir = PathBuf::from("/steps/").join(&pkg).join("build");
            if let Some(unpack) = unpack {
                for i in unpack {
                    match i {
                        RecipeBuildStep::Simple(step) if step == "default" => {
                            do_step(
                                &RecipeBuildStep::Simple(
                                    "bash -exc '. /steps/helpers.sh; default_src_unpack'"
                                        .to_owned(),
                                ),
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                        RecipeBuildStep::Complex { cmd, serial, bash } if cmd == "default" => {
                            do_step(
                                &RecipeBuildStep::Complex {
                                    cmd: "bash -exc '. /steps/helpers.sh; default_src_unpack'"
                                        .to_owned(),
                                    serial,
                                    bash,
                                },
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                        i => {
                            do_step(
                                &i,
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                    }
                }
            } else {
                do_step(
                    &RecipeBuildStep::Simple(
                        "bash -exc '. /steps/helpers.sh; default_src_unpack'".to_owned(),
                    ),
                    &mut env_data,
                    &mut cur_dir,
                    &work_dir,
                    &status_updates,
                    &mut join_handles,
                );
                env_data.insert("dirname".to_owned(), unpack_dirname.clone());
                cur_dir = cur_dir.join(unpack_dirname);
            }
            if let Some(prepare) = prepare {
                for i in prepare {
                    match i {
                        RecipeBuildStep::Simple(step) if step == "default" => {
                            do_step(
                                &RecipeBuildStep::Simple(
                                    "bash -exc '. /steps/helpers.sh; default_src_prepare'"
                                        .to_owned(),
                                ),
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                        RecipeBuildStep::Complex { cmd, serial, bash } if cmd == "default" => {
                            do_step(
                                &RecipeBuildStep::Complex {
                                    cmd: "bash -exc '. /steps/helpers.sh; default_src_prepare'"
                                        .to_owned(),
                                    serial,
                                    bash,
                                },
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                        i => {
                            do_step(
                                &i,
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                    }
                }
            } else {
                do_step(
                    &RecipeBuildStep::Simple(
                        "bash -exc '. /steps/helpers.sh; default_src_prepare'".to_owned(),
                    ),
                    &mut env_data,
                    &mut cur_dir,
                    &work_dir,
                    &status_updates,
                    &mut join_handles,
                );
            }
            if let Some(configure) = configure {
                for i in configure {
                    match i {
                        RecipeBuildStep::Simple(step) if step == "default" => {
                            do_step(
                                &RecipeBuildStep::Simple(
                                    "bash -exc '. /steps/helpers.sh; default_src_configure'"
                                        .to_owned(),
                                ),
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                        RecipeBuildStep::Complex { cmd, serial, bash } if cmd == "default" => {
                            do_step(
                                &RecipeBuildStep::Complex {
                                    cmd: "bash -exc '. /steps/helpers.sh; default_src_configure'"
                                        .to_owned(),
                                    serial,
                                    bash,
                                },
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                        i => {
                            do_step(
                                &i,
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                    }
                }
            } else {
                do_step(
                    &RecipeBuildStep::Simple(
                        "bash -exc '. /steps/helpers.sh; default_src_configure'".to_owned(),
                    ),
                    &mut env_data,
                    &mut cur_dir,
                    &work_dir,
                    &status_updates,
                    &mut join_handles,
                );
            }
            if let Some(compile) = compile {
                for i in compile {
                    match i {
                        RecipeBuildStep::Simple(step) if step == "default" => {
                            do_step(
                                &RecipeBuildStep::Simple(
                                    "bash -exc '. /steps/helpers.sh; default_src_compile'"
                                        .to_owned(),
                                ),
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                        RecipeBuildStep::Complex { cmd, serial, bash } if cmd == "default" => {
                            do_step(
                                &RecipeBuildStep::Complex {
                                    cmd: "bash -exc '. /steps/helpers.sh; default_src_compile'"
                                        .to_owned(),
                                    serial,
                                    bash,
                                },
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                        i => {
                            do_step(
                                &i,
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                    }
                }
            } else {
                do_step(
                    &RecipeBuildStep::Simple(
                        "bash -exc '. /steps/helpers.sh; default_src_compile'".to_owned(),
                    ),
                    &mut env_data,
                    &mut cur_dir,
                    &work_dir,
                    &status_updates,
                    &mut join_handles,
                );
            }
            create_dir_all(work_dir.path().join(sanitize_path(&PathBuf::from(
                env_data.get("DESTDIR").unwrap(),
            ))))
            .unwrap();
            if let Some(install) = install {
                for i in install {
                    match i {
                        RecipeBuildStep::Simple(step) if step == "default" => {
                            do_step(
                                &RecipeBuildStep::Simple(
                                    "bash -exc '. /steps/helpers.sh; default_src_install'"
                                        .to_owned(),
                                ),
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                        RecipeBuildStep::Complex { cmd, serial, bash } if cmd == "default" => {
                            do_step(
                                &RecipeBuildStep::Complex {
                                    cmd: "bash -exc '. /steps/helpers.sh; default_src_install'"
                                        .to_owned(),
                                    serial,
                                    bash,
                                },
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                        i => {
                            do_step(
                                &i,
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                    }
                }
            } else {
                do_step(
                    &RecipeBuildStep::Simple(
                        "bash -exc '. /steps/helpers.sh; default_src_install'".to_owned(),
                    ),
                    &mut env_data,
                    &mut cur_dir,
                    &work_dir,
                    &status_updates,
                    &mut join_handles,
                );
            }
            if let Some(postprocess) = postprocess {
                for i in postprocess {
                    match i {
                        RecipeBuildStep::Simple(step) if step == "default" => {
                            do_step(
                                &RecipeBuildStep::Simple(
                                    "bash -exc '. /steps/helpers.sh; default_src_postprocess'"
                                        .to_owned(),
                                ),
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                        RecipeBuildStep::Complex { cmd, serial, bash } if cmd == "default" => {
                            do_step(
                                &RecipeBuildStep::Complex {
                                    cmd: "bash -exc '. /steps/helpers.sh; default_src_postprocess'"
                                        .to_owned(),
                                    serial,
                                    bash,
                                },
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                        i => {
                            do_step(
                                &i,
                                &mut env_data,
                                &mut cur_dir,
                                &work_dir,
                                &status_updates,
                                &mut join_handles,
                            );
                        }
                    }
                }
            } else {
                do_step(
                    &RecipeBuildStep::Simple(
                        "bash -exc '. /steps/helpers.sh; default_src_postprocess'".to_owned(),
                    ),
                    &mut env_data,
                    &mut cur_dir,
                    &work_dir,
                    &status_updates,
                    &mut join_handles,
                );
            }
            cur_dir = PathBuf::from(env_data.get("DESTDIR").unwrap());
            do_step(
                &RecipeBuildStep::Simple("bash -exc '. /steps/helpers.sh; src_pkg'".to_owned()),
                &mut env_data,
                &mut cur_dir,
                &work_dir,
                &status_updates,
                &mut join_handles,
            );
            cur_dir = PathBuf::from("/external/repo");
            do_step(
                &RecipeBuildStep::Simple(
                    "bash -exc '. /steps/helpers.sh; src_checksum ${pkg} ${revision}'".to_owned(),
                ),
                &mut env_data,
                &mut cur_dir,
                &work_dir,
                &status_updates,
                &mut join_handles,
            );
        }
    }
    join_handles
        .drain(..)
        .for_each(|mut res| assert!(res.wait().unwrap().success()));
    let repo_dir = tempfile::tempdir_in("ramdir/").unwrap();
    if !recipe.artefacts.is_empty() && recipe.artefacts[0].ends_with(".tar.bz2") {
        let mut tar = tar::Archive::new(BzDecoder::new(
            std::fs::File::open(
                work_dir
                    .path()
                    .join(sanitize_path(&PathBuf::from(recipe.artefacts[0].clone()))),
            )
            .unwrap(),
        ));
        tar.unpack(repo_dir.path()).unwrap();
    }
    let mut tar_writer = tar::Builder::new(Cursor::new(Vec::new()));
    tar_writer.mode(tar::HeaderMode::TimestampDeterministic);
    tar_writer.follow_symlinks(false);
    let curdir = std::env::current_dir().unwrap();
    std::env::set_current_dir(work_dir.path()).unwrap();
    for artefact in recipe.artefacts {
        let trimmed_path = sanitize_path(&PathBuf::from(artefact));
        if std::fs::symlink_metadata(&trimmed_path)
            .unwrap_or_else(|_| panic!("{:?}", trimmed_path))
            .is_dir()
        {
            tar_writer
                .append_dir_all(&trimmed_path, &trimmed_path)
                .unwrap();
        } else {
            tar_writer.append_path(trimmed_path).unwrap();
        }
    }
    std::env::set_current_dir(repo_dir.path()).unwrap();
    for i in read_dir(repo_dir.path()).unwrap() {
        let i = i.unwrap();
        if i.path().is_dir() {
            tar_writer
                .append_dir_all(i.file_name(), i.file_name())
                .unwrap();
        } else {
            tar_writer.append_path(i.file_name()).unwrap()
        }
    }
    std::env::set_current_dir(curdir).unwrap();
    umount(&work_dir.path().join("proc")).unwrap();
    umount(&work_dir.path().join("dev/pts")).unwrap();
    tar_writer.finish().unwrap();
    let tar_buf = tar_writer.into_inner().unwrap().into_inner();
    let hash = sha256::digest(&tar_buf);
    status_updates.send(StatusUpdate::Done).unwrap();
    return (hash, tar_buf);
}
