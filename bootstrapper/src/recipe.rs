use std::{
    collections::BTreeMap,
    fs::File,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    str::FromStr,
};

use lazy_static::lazy_static;

use serde::{Deserialize, Serialize};

lazy_static! {
    pub static ref SOURCES: BTreeMap<String, SourceContents> = load_sources();
    static ref EQUIV_CACHE: lockfree::map::Map<(String, String), String> =
        lockfree::map::Map::new();
}

pub fn load_sources() -> BTreeMap<String, SourceContents> {
    serde_yaml::from_reader::<File, BTreeMap<String, SourceContents>>(
        File::open("sources.yaml").unwrap(),
    )
    .unwrap()
}

pub fn get_recipe_hash(name: &str, version: &str, salt: &str) -> String {
    sha256::digest(
        bincode::serialize(&NamedRecipeVersion::load_by_target_version(name, version)).unwrap(),
    )
}

pub fn get_depd_hash(name: &str, version: &str, salt: &str) -> Option<String> {
    let mut recipe_hash = get_recipe_hash(&name, &version, salt);
    for dep in NamedRecipeVersion::load_by_target_version(name, version)
        .deps
        .unwrap_or_default()
    {
        recipe_hash.push(',');
        if let Some(equiv) = get_equiv_hash(&dep.name, &dep.version, salt) {
            recipe_hash.push_str(&equiv);
        } else {
            return None;
        }
    }
    Some(sha256::digest(recipe_hash))
}

pub fn get_equiv_hash(name: &str, version: &str, salt: &str) -> Option<String> {
    if let Some(hash) = EQUIV_CACHE.get(&(name.to_owned(), version.to_owned())) {
        return Some(hash.1.clone());
    }
    if let Some(depd_hash) = get_depd_hash(name, version, salt) {
        let db: sled::Db = sled::open("equiv.sled").unwrap();
        let res = db
            .get(depd_hash)
            .unwrap()
            .map(|x| String::from_utf8(x.to_vec()).unwrap());
        if let Some(hash) = &res {
            EQUIV_CACHE.insert((name.to_owned(), version.to_owned()), hash.to_owned());
        }
        res
    } else {
        None
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Source {
    pub extract: Option<String>,
    pub noextract: Option<String>,
    pub copy: Option<Vec<String>>,
    pub chmod: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SourceContents {
    pub url: String,
    pub sha: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum RecipeBuildSteps {
    Single {
        single: Vec<RecipeBuildStep>,
    },
    Piecewise {
        unpack: Option<Vec<RecipeBuildStep>>,
        unpack_dirname: String,
        patch_dir: String,
        package_dir: Option<String>,
        prepare: Option<Vec<RecipeBuildStep>>,
        configure: Option<Vec<RecipeBuildStep>>,
        compile: Option<Vec<RecipeBuildStep>>,
        install: Option<Vec<RecipeBuildStep>>,
        postprocess: Option<Vec<RecipeBuildStep>>,
    },
}

const fn _default_true() -> bool {
    true
}

const fn _default_false() -> bool {
    false
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum RecipeBuildStep {
    Simple(String),
    Complex {
        cmd: String,
        #[serde(default = "_default_true")]
        serial: bool,
        #[serde(default = "_default_false")]
        bash: bool,
    },
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DepSpec {
    pub name: String,
    pub version: String,
    pub from: Option<String>,
    pub to: Option<String>,
}

impl From<String> for DepSpec {
    fn from(s: String) -> Self {
        let mut dep_iter = s.split(':');
        let name = dep_iter.next().unwrap().to_string();
        let version = dep_iter.next().unwrap().to_string();
        let from = dep_iter.next().map(str::to_string);
        let to = dep_iter.next().map(str::to_string);
        Self {
            name,
            version,
            from,
            to,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum Owner {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Debug, Deserialize, Clone)]
pub struct License {
    spdx: String,
    owner: Owner,
    license_file: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Licenses {
    recipe: Option<License>,
    package: Option<License>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RecipeVersion {
    pub licenses: Option<Licenses>,
    pub source: Option<BTreeMap<String, Source>>,
    pub shell: Option<String>,
    pub deps: Option<Vec<String>>,
    pub mkdirs: Option<Vec<String>>,
    pub build: RecipeBuildSteps,
    pub artefacts: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NamedRecipeVersion {
    pub name: String,
    pub version: String,
    pub source: Option<BTreeMap<String, Source>>,
    pub shell: Option<DepSpec>,
    pub deps: Option<Vec<DepSpec>>,
    pub mkdirs: Option<Vec<String>>,
    pub build: RecipeBuildSteps,
    pub artefacts: Vec<String>,
}

impl NamedRecipeVersion {
    pub fn load_by_name(name: &str) -> Self {
        let (target, version) = name.split_once(':').unwrap();
        Self::load_by_target_version(target, version)
    }
    pub fn load_by_target_version(target: &str, version: &str) -> Self {
        let rv: RecipeVersion = serde_yaml::from_reader(std::fs::File::open(PathBuf::from("recipes").join(target).join(format!("{}.yaml",version))).unwrap()).unwrap();
        NamedRecipeVersion {
            name: target.to_owned(),
            version: version.to_owned(),
            source: rv.source,
            shell: rv.shell.map(|x| x.into()),
            deps: rv.deps.map(|x| x.into_iter().map(|x| x.into()).collect()),
            mkdirs: rv.mkdirs,
            build: rv.build,
            artefacts: rv.artefacts,
        }
    }
}
