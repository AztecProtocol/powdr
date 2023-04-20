use std::collections::BTreeMap;
use std::{path::Path, process::Command};

use mktemp::Temp;
use std::fs;
use walkdir::WalkDir;

use crate::number::FieldElement;

pub mod compiler;
pub mod parser;

/// Compiles a rust file all the way down to PIL and generates
/// fixed and witness columns.
pub fn compile_rust(
    file_name: &str,
    full_crate: bool,
    inputs: Vec<FieldElement>,
    output_dir: &Path,
    force_overwrite: bool,
) {
    let riscv_asm = if full_crate {
        let cargo_toml = if file_name.ends_with("Cargo.toml") {
            file_name.to_string()
        } else {
            format!("{file_name}/Cargo.toml")
        };
        compile_rust_crate_to_riscv_asm(&cargo_toml)
    } else {
        compile_rust_to_riscv_asm(file_name)
    };
    let riscv_asm_file_name = output_dir.join(format!(
        "{}_riscv.asm",
        Path::new(file_name).file_stem().unwrap().to_str().unwrap()
    ));
    if riscv_asm_file_name.exists() && !force_overwrite {
        eprint!(
            "Target file {} already exists. Not overwriting.",
            riscv_asm_file_name.to_str().unwrap()
        );
        return;
    }

    let merged = riscv_asm
        .iter()
        .fold(String::default(), |acc, v| format!("{acc}\n{}", v.1));

    fs::write(riscv_asm_file_name.clone(), merged).unwrap();
    log::info!("Wrote {}", riscv_asm_file_name.to_str().unwrap());

    compile_riscv_asm_bundle(file_name, riscv_asm, inputs, output_dir, force_overwrite)
}

pub fn compile_riscv_asm_bundle(
    original_file_name: &str,
    files: BTreeMap<String, String>,
    inputs: Vec<FieldElement>,
    output_dir: &Path,
    force_overwrite: bool,
) {
    let powdr_asm_file_name = output_dir.join(format!(
        "{}.asm",
        Path::new(original_file_name)
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
    ));
    if powdr_asm_file_name.exists() && !force_overwrite {
        eprint!(
            "Target file {} already exists. Not overwriting.",
            powdr_asm_file_name.to_str().unwrap()
        );
        return;
    }

    let powdr_asm = files.iter().fold(String::new(), |acc, file| {
        format!("{acc}\n\n{}", compiler::compile_riscv_asm(&file.0, &file.1))
    });

    fs::write(powdr_asm_file_name.clone(), &powdr_asm).unwrap();
    log::info!("Wrote {}", powdr_asm_file_name.to_str().unwrap());

    crate::compiler::compile_asm_string(
        powdr_asm_file_name.to_str().unwrap(),
        &powdr_asm,
        inputs,
        output_dir,
        force_overwrite,
    )
}

/// Compiles a riscv asm file all the way down to PIL and generates
/// fixed and witness columns.
/// Adds required library routines automatically.
pub fn compile_riscv_asm(
    original_file_name: &str,
    file_name: &str,
    inputs: Vec<FieldElement>,
    output_dir: &Path,
    force_overwrite: bool,
) {
    let contents = fs::read_to_string(file_name).unwrap();
    let powdr_asm = compiler::compile_riscv_asm(&original_file_name, &contents);
    let powdr_asm_file_name = output_dir.join(format!(
        "{}.asm",
        Path::new(original_file_name)
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
    ));
    if powdr_asm_file_name.exists() && !force_overwrite {
        eprint!(
            "Target file {} already exists. Not overwriting.",
            powdr_asm_file_name.to_str().unwrap()
        );
        return;
    }
    fs::write(powdr_asm_file_name.clone(), &powdr_asm).unwrap();
    log::info!("Wrote {}", powdr_asm_file_name.to_str().unwrap());

    crate::compiler::compile_asm_string(
        powdr_asm_file_name.to_str().unwrap(),
        &powdr_asm,
        inputs,
        output_dir,
        force_overwrite,
    )
}

pub fn compile_rust_to_riscv_asm(input_file: &str) -> BTreeMap<String, String> {
    let crate_dir = Temp::new_dir().unwrap();
    // TODO is there no easier way?
    let mut cargo_file = crate_dir.clone();
    cargo_file.push("Cargo.toml");

    fs::write(
        &cargo_file,
        format!(
            r#"[package]
name = "{}"
version = "0.1.0"
edition = "2021"
            "#,
            Path::new(input_file).file_stem().unwrap().to_str().unwrap()
        ),
    )
    .unwrap();

    let mut src_file = crate_dir.clone();
    src_file.push("src");
    fs::create_dir(&src_file).unwrap();
    src_file.push("lib.rs");
    fs::write(src_file, fs::read_to_string(input_file).unwrap()).unwrap();

    compile_rust_crate_to_riscv_asm(cargo_file.to_str().unwrap())
}

pub fn compile_rust_crate_to_riscv_asm(input_dir: &str) -> BTreeMap<String, String> {
    let temp_dir = Temp::new_dir().unwrap();

    let cargo_status = Command::new("cargo")
        .env("RUSTFLAGS", "--emit=asm")
        .args([
            "build",
            "--release",
            "-Z",
            "build-std=core",
            "--target",
            "riscv32imc-unknown-none-elf",
            "--lib",
            "--target-dir",
            temp_dir.to_str().unwrap(),
            "--manifest-path",
            input_dir,
        ])
        .status()
        .unwrap();
    assert!(cargo_status.success());

    let mut all_asm = BTreeMap::<String, String>::default();
    for entry in WalkDir::new(&temp_dir) {
        let entry = entry.unwrap();
        // TODO search only in certain subdir?
        if entry.file_name().to_str().unwrap().ends_with(".s") {
            all_asm.insert(
                entry
                    .path()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string(),
                fs::read_to_string(entry.path()).unwrap().clone(),
            );
        }
    }

    all_asm
}
