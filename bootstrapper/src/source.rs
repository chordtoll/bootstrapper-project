use std::path::PathBuf;

use crate::recipe::SourceContents;

pub fn source_path(hash: &str) -> PathBuf {
    PathBuf::from("build-cache")
        .join("source")
        .join(hash[0..2].to_string())
        .join(hash[2..4].to_string())
        .join(hash)
}

pub fn fetch_source(source: &SourceContents) -> Vec<u8> {
    println!("Downloading {}",source.url);
    let source_data = reqwest::blocking::get(&source.url).unwrap().bytes().unwrap();
    assert_eq!(source.sha, sha256::digest(&*source_data));
    let store_path = source_path(&source.sha);
    std::fs::create_dir_all(store_path.parent().unwrap()).unwrap();
    std::fs::write(store_path,&source_data).unwrap();
    source_data.to_vec()
}