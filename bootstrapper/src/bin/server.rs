use bootstrapper::{
    env_substitute, network::{
        finish_deps, finish_overlays, finish_sources, write_dep, write_envs, write_overlay,
        write_source,
    }, recipe::{get_depd_hash, get_equiv_hash, NamedRecipeVersion, RecipeVersion, SOURCES}, source::{fetch_source, source_path}, WorkerStatus
};
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use lazy_static::lazy_static;
use maplit::btreemap;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
};
use walkdir::WalkDir;

lazy_static! {
    static ref WORK_QUEUE: atomic_queue::Queue<String> = atomic_queue::bounded(10);
}

fn ready_to_build(
    deptree: &BTreeMap<(String, String), BTreeSet<(String, String)>>,
) -> BTreeSet<&(String, String)> {
    deptree
        .iter()
        .filter_map(|(k, v)| if v.is_empty() { Some(k) } else { None })
        .collect()
}
fn finish_dep(
    deptree: &mut BTreeMap<(String, String), BTreeSet<(String, String)>>,
    dep: &(String, String),
) {
    deptree.remove(dep);
    deptree.values_mut().for_each(|v| {
        v.remove(dep);
    });
}

fn dep_path(hash: &str) -> PathBuf {
    PathBuf::from("build-cache")
        .join("build")
        .join(hash[0..2].to_string())
        .join(hash[2..4].to_string())
        .join(hash)
}

fn test_dep(dep: &(String, String)) -> bool {
    println!(" Testing for prebuilt {:?}...", dep);
    let equiv_hash = get_equiv_hash(&dep.0.clone(), &dep.1.clone(), "");
    println!("  Equivalent to {:?}", equiv_hash);
    if let Some(hash) = equiv_hash {
        std::fs::exists(dep_path(&hash)).unwrap()
    } else {
        false
    }
}

fn load_dep(dep: &(String, String)) -> Vec<u8> {
    let equiv_hash = get_equiv_hash(&dep.0.clone(), &dep.1.clone(), "").unwrap();
    std::fs::read(dep_path(&equiv_hash)).unwrap()
}

fn store_dep(dep: &(String, String), contents: &[u8]) {
    let recipe_hash = get_depd_hash(&dep.0, &dep.1, "").unwrap();
    let equiv_hash = sha256::digest(contents);
    let dep_path = dep_path(&equiv_hash);
    std::fs::create_dir_all(dep_path.parent().unwrap()).unwrap();
    std::fs::write(dep_path, contents).unwrap();
    let db: sled::Db = sled::open("equiv.sled").unwrap();
    db.insert(recipe_hash, equiv_hash.as_str()).unwrap();
}

fn main() {
    println!("Loading recipes...");
    let mut recipes = BTreeMap::new();
    for entry in glob::glob("recipes/*/**/*.yaml").unwrap() {
        let entry = entry.unwrap();
        let name = entry
            .parent()
            .unwrap()
            .strip_prefix("recipes")
            .unwrap()
            .as_os_str()
            .to_str()
            .unwrap();
        let version = entry
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .trim_end_matches(".yaml");
        let recipe: RecipeVersion =
            serde_yaml::from_reader(File::open(entry.clone()).unwrap()).unwrap();


        if recipe.licenses.is_none() {
            println!("No license for {}:{}",name,version);
        }

        match recipes.entry(name.to_owned()) {
            std::collections::btree_map::Entry::Vacant(vacant_entry) => {
                vacant_entry.insert(btreemap! {version.to_owned()=>recipe});
            }
            std::collections::btree_map::Entry::Occupied(mut occupied_entry) => {
                occupied_entry.get_mut().insert(version.to_owned(), recipe);
            }
        }
    }

    let mut deptree = BTreeMap::new();
    for (name, i) in recipes.iter() {
        for (ver, _) in i.iter() {
            //TODO this is inefficient, we're loading twice
            let recipe = NamedRecipeVersion::load_by_target_version(name, ver);
            let depset = if let Some(deps) = &recipe.deps {
                deps.iter()
                    .map(|x| (x.name.clone(), x.version.clone()))
                    .collect()
            } else {
                BTreeSet::new()
            };
            deptree.insert((name.to_owned(), ver.to_owned()), depset);
        }
    }

    let listener = TcpListener::bind("0.0.0.0:1234").unwrap();
    println!("Waiting for worker...");
    let (mut stream, _) = listener.accept().unwrap();

    while let Some(to_build) = ready_to_build(&deptree).first() {
        let to_build = (*to_build).clone();

        println!("Considering {:?}", to_build);

        if test_dep(&to_build) {
            finish_dep(&mut deptree, &to_build);
            continue;
        }

        println!(" Dispatching {:?}", to_build);

        let archive_buf = build_recipe(&mut stream, to_build.clone());

        store_dep(&to_build, &archive_buf);

        finish_dep(&mut deptree, &to_build);
    }

    assert_eq!(stream.read_u8().unwrap(), WorkerStatus::ReadyForWork as u8);

    stream.write_u8(1).unwrap();

    if !deptree.is_empty() {
        println!("Remaining packages:");
        for ((name,ver),deps) in deptree.iter() {
            //if deps.len() == 1 {
                println!("{}:{} -> {:?}",name,ver,deps);
            //}
        }
    }
}

fn build_recipe(stream: &mut TcpStream, to_build: (String, String)) -> Vec<u8> {
    assert_eq!(stream.read_u8().unwrap(), WorkerStatus::ReadyForWork as u8);

    stream.write_u8(0).unwrap();

    let recipe = NamedRecipeVersion::load_by_target_version(&to_build.0, &to_build.1);

    let recipe_ser = serde_yaml::to_string(&recipe).unwrap().as_bytes().to_vec();
    stream
        .write_u64::<BigEndian>(recipe_ser.len().try_into().unwrap())
        .unwrap();
    stream.write_all(&recipe_ser).unwrap();

    if let Some(sources) = recipe.source {
        for (name, _) in sources {
            let source_contents = SOURCES.get(&name).unwrap();
            let spath = source_path(&source_contents.sha);
            let source_data = if spath.exists() {
                std::fs::read(spath).unwrap()
            } else {
                fetch_source(source_contents)
            };
            write_source(stream, &name, source_contents, &source_data);
        }
    }
    finish_sources(stream);

    if let Some(deps) = recipe.deps {
        for dep in deps {
            write_dep(
                stream,
                &format!("{}:{}", dep.name, dep.version),
                &load_dep(&(dep.name, dep.version)),
            );
        }
    }
    finish_deps(stream);

    let overlay_path = PathBuf::from(format!("recipes/{}/{}", to_build.0, to_build.1));
    if overlay_path.exists() {
        for entry in WalkDir::new(&overlay_path) {
            let entry = entry.unwrap();
            if entry.metadata().unwrap().is_file() {
                write_overlay(
                    stream,
                    entry.path().strip_prefix(&overlay_path).unwrap(),
                    &std::fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    finish_overlays(stream);

    let mut dir_envs = BTreeMap::new();
    if let Ok(v) = std::fs::read(
        PathBuf::from(format!("recipes/{}.yaml", to_build.0))
            .parent()
            .unwrap()
            .join("env"),
    ) {
        for line in String::from_utf8(v).unwrap().split('\n') {
            let (k, v) = line.split_once('=').unwrap();
            dir_envs.insert(k.to_owned(), env_substitute(v.trim_matches('"'), &dir_envs));
        }
    };

    write_envs(stream, dir_envs);

    assert_eq!(stream.read_u8().unwrap(), WorkerStatus::BuildComplete as u8);
    let mut hash = vec![0u8; 64];
    stream.read_exact(hash.as_mut_slice()).unwrap();
    let archive_len = stream.read_u64::<byteorder::BigEndian>().unwrap();
    let mut archive_buf = vec![0u8; archive_len.try_into().unwrap()];
    stream.read_exact(archive_buf.as_mut_slice()).unwrap();

    archive_buf
}
