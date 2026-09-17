use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=native/voicepeak_resident.cpp");
    println!("cargo:rerun-if-env-changed=CXX");
    if env::var("TARGET").unwrap() == "x86_64-unknown-linux-gnu" {
        let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("voicepeak_resident.so");
        let status = Command::new(env::var_os("CXX").unwrap_or_else(|| "c++".into()))
            .args(["-std=c++17", "-O2", "-shared", "-fPIC", "-pthread"])
            .arg("native/voicepeak_resident.cpp")
            .arg("-o")
            .arg(output)
            .status()
            .expect("C++ compiler is required for Linux VOICEPEAK resident support");
        assert!(status.success(), "VOICEPEAK resident library build failed");
    }
}
