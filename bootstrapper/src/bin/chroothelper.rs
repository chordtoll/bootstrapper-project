use std::{collections::BTreeMap, ffi::OsString, os::unix::ffi::OsStringExt, path::PathBuf};

use base64::{engine::general_purpose::URL_SAFE, Engine};

fn main() {
    let mut args = std::env::args();
    args.next().unwrap();
    let builddir = PathBuf::from(OsString::from_vec(
        URL_SAFE.decode(args.next().unwrap()).unwrap(),
    ));
    let chdir = PathBuf::from(OsString::from_vec(
        URL_SAFE.decode(args.next().unwrap()).unwrap(),
    ));
    let mut command: Vec<String> =
        serde_yaml::from_slice(&URL_SAFE.decode(args.next().unwrap()).unwrap()).unwrap();
    let environ: BTreeMap<String, String> =
        serde_yaml::from_slice(&URL_SAFE.decode(args.next().unwrap()).unwrap()).unwrap();
    assert!(args.next().is_none());
    std::os::unix::fs::chroot(builddir).unwrap();
    std::env::set_current_dir("/").unwrap();
    println!("CD {:?}", chdir);
    std::env::set_current_dir(chdir).unwrap();
    let command_executable = command.remove(0);
    println!("RUN {:?}", command_executable);
    assert!(std::process::Command::new(command_executable)
        .args(command)
        .envs(environ)
        .spawn()
        .unwrap()
        .wait()
        .unwrap()
        .success())
}
