// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

// build.rs

use std::{fs::OpenOptions, io::BufWriter, path::Path, process::Command};

// https://github.com/sagiegurari/cargo-make
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Building....");

    let out_dir = std::env::var("OUT_DIR").expect("no out dir");
    let out_dir = Path::new(&out_dir);
    let terminfo_out_dir = out_dir.join("terminfo");
    let terminfo_definition = "../res/freminal.ti";
    println!("OUT_DIR: {:?}", out_dir);
    println!("cargo:rerun-if-changed={terminfo_definition}");

    #[cfg(not(target_family = "windows"))]
    {
        println!("cargo:warning=Compiling terminfo");
        let mut child = Command::new("tic")
            .arg("-o")
            .arg(&terminfo_out_dir)
            .arg("-x")
            .arg(terminfo_definition)
            .spawn()
            .expect("Failed to spawn terminfo compiler");
        let status = child
            .wait()
            .expect("failed to get terminfo compiler result");
        assert!(status.success());
    }

    #[cfg(target_family = "windows")]
    {
        println!("cargo:warning=Windows detected, skipping terminfo compilation");
        // copy the precompiled terminfo to the output directory
        // ../res/terminfo has the precompiled terminfo

        let terminfo_dir = out_dir.join("terminfo");
        std::fs::create_dir_all(&terminfo_dir).expect("Failed to create terminfo directory");
        let terminfo_files = std::fs::read_dir("../res/terminfo")
            .expect("Failed to read terminfo directory")
            .map(|entry| entry.expect("Failed to read terminfo file").path())
            .collect::<Vec<_>>();
        for terminfo_file in terminfo_files {
            let terminfo_file_name = terminfo_file.file_name().unwrap();
            let terminfo_file_out = terminfo_dir.join(terminfo_file_name);

            // if the entry is a folder, make it

            if terminfo_file.is_dir() {
                std::fs::create_dir_all(&terminfo_file_out)
                    .expect("Failed to create terminfo directory");
            } else {
                std::fs::copy(&terminfo_file, &terminfo_file_out)
                    .expect("Failed to copy terminfo file");
            }
        }
    }

    let terminfo_tarball_path = out_dir.join("terminfo.tar");
    let terminfo_tarball_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(terminfo_tarball_path)
        .expect("Failed to open terminfo tarball for writing");

    let mut tar_builder = tar::Builder::new(BufWriter::new(terminfo_tarball_file));
    tar_builder
        .append_dir_all(".", terminfo_out_dir)
        .expect("Failed to add terminfo to tarball");
    tar_builder
        .finish()
        .expect("Failed to write terminfo tarball");
    Ok(())
}
