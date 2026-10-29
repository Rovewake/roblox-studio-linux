use std::{env, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let repository_root = manifest_dir.join("../..");
    let luau_source = repository_root.join("vendor/luau");

    if !luau_source.join("CMakeLists.txt").is_file() {
        panic!(
            "Luau source submodule is missing; clone with `git clone --recurse-submodules` or run `git submodule update --init --recursive`"
        );
    }

    println!("cargo:rerun-if-changed=native/CMakeLists.txt");
    println!("cargo:rerun-if-changed=native/bridge.cpp");
    println!("cargo:rerun-if-changed=native/bridge.h");
    println!("cargo:rerun-if-changed=native/engine_definitions.cpp.in");
    println!("cargo:rerun-if-changed=native/definitions/roblox-engine-0.745.0.7451137.luau");
    println!("cargo:rerun-if-changed={}", luau_source.display());

    let destination = cmake::Config::new(manifest_dir.join("native"))
        .define("LUAU_SOURCE_DIR", &luau_source)
        .define("LUAU_BUILD_CLI", "OFF")
        .define("LUAU_BUILD_TESTS", "OFF")
        .define("LUAU_BUILD_WEB", "OFF")
        .define("LUAU_BUILD_SHARED", "OFF")
        .build();

    println!(
        "cargo:rustc-link-search=native={}",
        destination.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=studio_luau_native");
    println!("cargo:rustc-link-lib=static=Luau.Analysis");
    println!("cargo:rustc-link-lib=static=Luau.Config");
    println!("cargo:rustc-link-lib=static=Luau.Compiler");
    println!("cargo:rustc-link-lib=static=Luau.VM");
    println!("cargo:rustc-link-lib=static=Luau.Ast");
    println!("cargo:rustc-link-lib=static=Luau.Bytecode");
    println!("cargo:rustc-link-lib=static=Luau.Common");
    println!("cargo:rustc-link-lib=dylib=stdc++");
}
