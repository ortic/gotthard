//! Embeds the icon and version information in gotthard.exe (`gotthard.rc`).

fn main() {
    println!("cargo:rerun-if-changed=gotthard.rc");
    println!("cargo:rerun-if-changed=../../assets/icon/gotthard.ico");
    let macros = [
        concat!("GOTTHARD_VERSION=\"", env!("CARGO_PKG_VERSION"), "\""),
        concat!("GOTTHARD_VERSION_MAJOR=", env!("CARGO_PKG_VERSION_MAJOR")),
        concat!("GOTTHARD_VERSION_MINOR=", env!("CARGO_PKG_VERSION_MINOR")),
        concat!("GOTTHARD_VERSION_PATCH=", env!("CARGO_PKG_VERSION_PATCH")),
    ];
    // Not built for Windows: nothing to do. On Windows a failing rc.exe fails
    // the build; a cross build without a resource compiler only warns.
    let result = embed_resource::compile("gotthard.rc", macros);
    if let embed_resource::CompilationResult::NotAttempted(missing) = &result {
        println!("cargo:warning=gotthard.exe is built without its icon: no {missing}");
    }
    result.manifest_optional().unwrap();
}
