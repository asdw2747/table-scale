use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    println!("cargo:rerun-if-changed=resources/table-scale.rc");
    println!("cargo:rerun-if-changed=resources/table-scale.ico");
    println!("cargo:rerun-if-changed=resources/table-scale.manifest");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let icon = Path::new("resources/table-scale.ico");
    let manifest = Path::new("resources/table-scale.manifest");
    let template = Path::new("resources/table-scale.rc");

    fs::copy(icon, out_dir.join("table-scale.ico"))
        .expect("could not copy the Windows icon into OUT_DIR");
    fs::copy(manifest, out_dir.join("table-scale.manifest"))
        .expect("could not copy the Windows manifest into OUT_DIR");

    // The resource compiler runs against a preprocessed copy in OUT_DIR, so
    // icon/manifest references must be relative to that directory.
    let resource = fs::read_to_string(template)
        .expect("could not read resources/table-scale.rc")
        .replace("resources/table-scale.ico", "table-scale.ico")
        .replace("resources/table-scale.manifest", "table-scale.manifest");
    let resource_path = out_dir.join("table-scale.rc");
    fs::write(&resource_path, resource).expect("could not write the generated resource file");

    match embed_resource::compile(&resource_path, embed_resource::NONE) {
        embed_resource::CompilationResult::Failed(error) => {
            panic!("Windows resource compilation failed: {error}");
        }
        embed_resource::CompilationResult::NotAttempted(error) => {
            println!("cargo:warning=Windows resources were not embedded: {error}");
        }
        embed_resource::CompilationResult::NotWindows | embed_resource::CompilationResult::Ok => {}
    }
}
