//! Embeds the icon and version information in gotthard.exe (`gotthard.rc`).

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=gotthard.rc");
    println!("cargo:rerun-if-changed=../../assets/icon/gotthard.ico");
    let macros = [
        concat!("GOTTHARD_VERSION=\"", env!("CARGO_PKG_VERSION"), "\""),
        concat!("GOTTHARD_VERSION_MAJOR=", env!("CARGO_PKG_VERSION_MAJOR")),
        concat!("GOTTHARD_VERSION_MINOR=", env!("CARGO_PKG_VERSION_MINOR")),
        concat!("GOTTHARD_VERSION_PATCH=", env!("CARGO_PKG_VERSION_PATCH")),
    ];
    // Not built for Windows: nothing to do. On Windows rc.exe or windres
    // compile the resources and a failure fails the build.
    let result = embed_resource::compile("gotthard.rc", macros);
    if let embed_resource::CompilationResult::NotAttempted(missing) = &result {
        // A cross build from Linux (`cargo zigbuild`) has no windres; zig
        // brings a resource compiler of its own, and its linker takes the
        // .res file as it is.
        let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
        if windows {
            match zig_rc(&macros) {
                Ok(res) => println!("cargo:rustc-link-arg-bins={}", res.display()),
                Err(e) => println!(
                    "cargo:warning=gotthard.exe is built without its icon: no {missing}, and zig rc: {e}"
                ),
            }
        }
        return;
    }
    result.manifest_optional().unwrap();
}

/// Compiles `gotthard.rc` with `zig rc` into `$OUT_DIR/gotthard.res`.
fn zig_rc(macros: &[&str]) -> Result<PathBuf, String> {
    let out =
        PathBuf::from(std::env::var("OUT_DIR").map_err(|e| e.to_string())?).join("gotthard.res");
    let mut command = Command::new("zig");
    command.arg("rc");
    for definition in macros {
        command.arg("/d").arg(definition);
    }
    let output = command
        .arg("/fo")
        .arg(&out)
        .arg("gotthard.rc")
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(out)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}
