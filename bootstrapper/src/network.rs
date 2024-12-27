use std::{
    collections::BTreeMap,
    ffi::OsStr,
    io::{Read, Write},
    net::TcpStream,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};

use crate::{
    recipe::{NamedRecipeVersion, SourceContents},
    WorkerStatus,
};

pub fn read_recipe(stream: &mut TcpStream) -> NamedRecipeVersion {
    let recipe_len = stream.read_u64::<byteorder::BigEndian>().unwrap();
    let mut recipe_buf = vec![0u8; recipe_len.try_into().unwrap()];
    stream.read_exact(recipe_buf.as_mut_slice()).unwrap();
    serde_yaml::from_slice(&recipe_buf).unwrap()
}

pub fn read_sources(stream: &mut TcpStream) -> BTreeMap<String, (SourceContents, Vec<u8>)> {
    let mut source_data = BTreeMap::new();

    loop {
        stream.write_u8(WorkerStatus::ReadyForSource as u8).unwrap();
        let source_name_len = stream.read_u16::<byteorder::BigEndian>().unwrap();
        if source_name_len == 0 {
            break;
        };
        let mut source_name_buf = vec![0u8; source_name_len.try_into().unwrap()];
        stream.read_exact(source_name_buf.as_mut_slice()).unwrap();
        let source_name = String::from_utf8(source_name_buf).unwrap();

        stream.write_u8(WorkerStatus::NeedSource as u8).unwrap();

        let source_contents_len = stream.read_u32::<byteorder::BigEndian>().unwrap();
        let mut source_contents_buf = vec![0u8; source_contents_len.try_into().unwrap()];
        stream
            .read_exact(source_contents_buf.as_mut_slice())
            .unwrap();
        let source_contents = serde_yaml::from_slice(&source_contents_buf).unwrap();

        let source_data_len = stream.read_u64::<byteorder::BigEndian>().unwrap();
        let mut source_data_buf = vec![0u8; source_data_len.try_into().unwrap()];
        stream.read_exact(source_data_buf.as_mut_slice()).unwrap();

        source_data.insert(source_name, (source_contents, source_data_buf));
    }
    source_data
}

pub fn write_source(stream: &mut TcpStream, name: &str, contents: &SourceContents, data: &[u8]) {
    assert_eq!(
        stream.read_u8().unwrap(),
        WorkerStatus::ReadyForSource as u8
    );
    let name = name.as_bytes().to_vec();
    stream
        .write_u16::<BigEndian>(name.len().try_into().unwrap())
        .unwrap();
    stream.write_all(&name).unwrap();
    assert_eq!(stream.read_u8().unwrap(), WorkerStatus::NeedSource as u8);
    let source_buf = serde_yaml::to_string(&contents)
        .unwrap()
        .as_bytes()
        .to_vec();
    stream
        .write_u32::<BigEndian>(source_buf.len().try_into().unwrap())
        .unwrap();
    stream.write_all(&source_buf).unwrap();
    stream
        .write_u64::<BigEndian>(data.len().try_into().unwrap())
        .unwrap();
    stream.write_all(&data).unwrap();
}

pub fn finish_sources(stream: &mut TcpStream) {
    assert_eq!(
        stream.read_u8().unwrap(),
        WorkerStatus::ReadyForSource as u8
    );
    stream.write_u16::<BigEndian>(0).unwrap();
}

pub fn read_deps(stream: &mut TcpStream) -> BTreeMap<String, Vec<u8>> {
    let mut dep_data = BTreeMap::new();

    loop {
        stream.write_u8(WorkerStatus::ReadyForDep as u8).unwrap();
        let dep_name_len = stream.read_u16::<byteorder::BigEndian>().unwrap();
        if dep_name_len == 0 {
            break;
        };
        let mut dep_name_buf = vec![0u8; dep_name_len.try_into().unwrap()];
        stream.read_exact(dep_name_buf.as_mut_slice()).unwrap();
        let dep_name = String::from_utf8(dep_name_buf).unwrap();

        stream.write_u8(WorkerStatus::NeedDep as u8).unwrap();

        let dep_data_len = stream.read_u64::<byteorder::BigEndian>().unwrap();
        let mut dep_data_buf = vec![0u8; dep_data_len.try_into().unwrap()];
        stream.read_exact(dep_data_buf.as_mut_slice()).unwrap();

        dep_data.insert(dep_name, dep_data_buf);
    }
    dep_data
}

pub fn write_dep(stream: &mut TcpStream, name: &str, data: &[u8]) {
    assert_eq!(stream.read_u8().unwrap(), WorkerStatus::ReadyForDep as u8);
    let name = name.as_bytes().to_vec();
    stream
        .write_u16::<BigEndian>(name.len().try_into().unwrap())
        .unwrap();
    stream.write_all(&name).unwrap();
    assert_eq!(stream.read_u8().unwrap(), WorkerStatus::NeedDep as u8);
    stream
        .write_u64::<BigEndian>(data.len().try_into().unwrap())
        .unwrap();
    stream.write_all(data).unwrap();
}

pub fn read_overlays(stream: &mut TcpStream) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut source_data = BTreeMap::new();

    loop {
        stream
            .write_u8(WorkerStatus::ReadyForOverlay as u8)
            .unwrap();
        let source_name_len = stream.read_u16::<byteorder::BigEndian>().unwrap();
        if source_name_len == 0 {
            break;
        };
        let mut source_name_buf = vec![0u8; source_name_len.try_into().unwrap()];
        stream.read_exact(source_name_buf.as_mut_slice()).unwrap();
        let source_name = PathBuf::from(OsStr::from_bytes(&source_name_buf));

        stream.write_u8(WorkerStatus::NeedOverlay as u8).unwrap();

        let source_data_len = stream.read_u64::<byteorder::BigEndian>().unwrap();
        let mut source_data_buf = vec![0u8; source_data_len.try_into().unwrap()];
        stream.read_exact(source_data_buf.as_mut_slice()).unwrap();

        source_data.insert(PathBuf::from(source_name), source_data_buf);
    }
    source_data
}

pub fn write_overlay(stream: &mut TcpStream, path: &Path, data: &[u8]) {
    assert_eq!(
        stream.read_u8().unwrap(),
        WorkerStatus::ReadyForOverlay as u8
    );
    let path = path.as_os_str().as_bytes().to_vec();
    stream
        .write_u16::<BigEndian>(path.len().try_into().unwrap())
        .unwrap();
    stream.write_all(&path).unwrap();
    assert_eq!(stream.read_u8().unwrap(), WorkerStatus::NeedOverlay as u8);
    stream
        .write_u64::<BigEndian>(data.len().try_into().unwrap())
        .unwrap();
    stream.write_all(&data).unwrap();
}

pub fn finish_overlays(stream: &mut TcpStream) {
    assert_eq!(
        stream.read_u8().unwrap(),
        WorkerStatus::ReadyForOverlay as u8
    );
    stream.write_u16::<BigEndian>(0).unwrap();
}

pub fn finish_deps(stream: &mut TcpStream) {
    assert_eq!(stream.read_u8().unwrap(), WorkerStatus::ReadyForDep as u8);
    stream.write_u16::<BigEndian>(0).unwrap();
}

pub fn read_envs(stream: &mut TcpStream) -> BTreeMap<String, String> {
    let mut env_data = BTreeMap::new();

    stream.write_u8(WorkerStatus::ReadyForEnvs as u8).unwrap();
    let env_count = stream.read_u16::<byteorder::BigEndian>().unwrap();
    for _ in 0..env_count {
        let env_k_len = stream.read_u16::<byteorder::BigEndian>().unwrap();
        let mut env_k_buf = vec![0u8; env_k_len.try_into().unwrap()];
        stream.read_exact(env_k_buf.as_mut_slice()).unwrap();
        let env_k = String::from_utf8(env_k_buf).unwrap();
        let env_v_len = stream.read_u16::<byteorder::BigEndian>().unwrap();
        let mut env_v_buf = vec![0u8; env_v_len.try_into().unwrap()];
        stream.read_exact(env_v_buf.as_mut_slice()).unwrap();
        let env_v = String::from_utf8(env_v_buf).unwrap();
        env_data.insert(env_k, env_v);
    }

    env_data
}

pub fn write_envs(stream: &mut TcpStream, envs: BTreeMap<String, String>) {
    assert_eq!(stream.read_u8().unwrap(), WorkerStatus::ReadyForEnvs as u8);
    stream
        .write_u16::<BigEndian>(envs.len().try_into().unwrap())
        .unwrap();
    for (k, v) in envs {
        let k = k.as_bytes().to_vec();
        stream
            .write_u16::<BigEndian>(k.len().try_into().unwrap())
            .unwrap();
        stream.write_all(&k).unwrap();
        let v = v.as_bytes().to_vec();
        stream
            .write_u16::<BigEndian>(v.len().try_into().unwrap())
            .unwrap();
        stream.write_all(&v).unwrap();
    }
}

pub fn write_archive(stream: &mut TcpStream, hash: &str, archive: &[u8]) {
    stream.write_u8(WorkerStatus::BuildComplete as u8).unwrap();
    assert_eq!(hash.as_bytes().len(), 64);
    stream.write_all(hash.as_bytes()).unwrap();
    stream
        .write_u64::<byteorder::BigEndian>(archive.len().try_into().unwrap())
        .unwrap();
    stream.write_all(&archive).unwrap();
}
