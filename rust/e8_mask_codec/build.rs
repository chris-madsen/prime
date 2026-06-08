use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(e8_mask_no_cuda)");
    println!("cargo:rerun-if-changed=cuda/e8_scan.cu");

    let has_nvcc = Command::new("bash")
        .arg("-lc")
        .arg("command -v nvcc >/dev/null 2>&1")
        .status()
        .map(|status| status.success())
        .unwrap_or(false);

    if !has_nvcc {
        println!("cargo:warning=nvcc not found; CUDA benchmark is disabled");
        println!("cargo:rustc-cfg=e8_mask_no_cuda");
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is not set"));
    let source = PathBuf::from("cuda/e8_scan.cu");
    let object = out_dir.join("e8_scan.o");
    let library = out_dir.join("libe8_mask_cuda.a");

    let compiled = Command::new("nvcc")
        .arg("-O3")
        .arg("-lineinfo")
        .arg("-Xcompiler")
        .arg("-fPIC")
        .arg("-std=c++14")
        .arg("-c")
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .status()
        .expect("failed to run nvcc");

    if !compiled.success() {
        println!("cargo:warning=CUDA kernel compilation failed; CUDA benchmark is disabled");
        println!("cargo:rustc-cfg=e8_mask_no_cuda");
        return;
    }

    let archived = Command::new("ar")
        .arg("crus")
        .arg(&library)
        .arg(&object)
        .status()
        .expect("failed to run ar");

    if !archived.success() {
        println!("cargo:warning=CUDA archive creation failed; CUDA benchmark is disabled");
        println!("cargo:rustc-cfg=e8_mask_no_cuda");
        return;
    }

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=e8_mask_cuda");
    println!("cargo:rustc-link-lib=dylib=cudart");
}
