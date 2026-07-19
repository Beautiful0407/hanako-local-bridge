use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=installer.rc");
    println!("cargo:rerun-if-changed=installer.exe.manifest");
    embed_resource::compile("installer.rc", embed_resource::NONE)
        .manifest_required()
        .expect("cannot embed installer application manifest");
    println!("cargo:rerun-if-env-changed=HANA_INSTALLER_PAYLOAD");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("payload.zip");
    if let Some(source) = env::var_os("HANA_INSTALLER_PAYLOAD") {
        let source = PathBuf::from(source);
        println!("cargo:rerun-if-changed={}", source.display());
        fs::copy(&source, &output).expect("cannot embed installer payload");
    } else {
        fs::write(&output, []).expect("cannot create empty installer payload");
    }
}
