use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTUAC:level='requireAdministrator' uiAccess='false'");
    const BUNDLE_DIRECTORY: &str = "MSI_WOL_SCEWIN_DIR";
    println!("cargo:rerun-if-env-changed={BUNDLE_DIRECTORY}");
    let source = env::var_os(BUNDLE_DIRECTORY).map(PathBuf::from);
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo provides OUT_DIR"));
    for name in ["SCEWIN_64.exe", "amifldrv64.sys", "amigendrv64.sys"] {
        let destination = output.join(name);
        if let Some(directory) = &source {
            let file = directory.join(name);
            println!("cargo:rerun-if-changed={}", file.display());
            fs::copy(file, destination).expect("could not include SCEWIN component");
        } else {
            fs::write(destination, []).expect("could not initialize empty SCEWIN bundle");
        }
    }
}
