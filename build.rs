// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Compiles the icons into a GResource that gets linked into the binary, so the
//! program can find its own icon before it is installed anywhere.

use std::path::Path;
use std::process::Command;

const SOURCE_DIR: &str = "data";
const MANIFEST: &str = "data/glance.gresource.xml";

fn main() {
    // qpdf, for writing PDFs with passwords and making them smaller. 11.0 is
    // the oldest with everything used; Ubuntu 24.04 has 11.9.
    if let Err(error) = pkg_config::Config::new().atleast_version("11.0").probe("libqpdf") {
        panic!(
            "qpdf 11 or newer is needed ({error}): install qpdf (Arch), qpdf-devel \
             (Fedora) or libqpdf-dev (Debian, Ubuntu)."
        );
    }

    let out_dir = std::env::var("OUT_DIR").expect("cargo always sets OUT_DIR");
    let target = Path::new(&out_dir).join("glance.gresource");

    // Rebuild when the manifest or any icon it names changes. Without these,
    // editing an icon would leave the old one baked into the binary.
    println!("cargo:rerun-if-changed={MANIFEST}");
    for icon in [
        "data/icons/hicolor/scalable/apps/io.github.abhilesh1412.Glance.svg",
        "data/icons/hicolor/symbolic/apps/io.github.abhilesh1412.Glance-symbolic.svg",
    ] {
        println!("cargo:rerun-if-changed={icon}");
    }

    let output = Command::new("glib-compile-resources")
        .arg("--sourcedir")
        .arg(SOURCE_DIR)
        .arg("--target")
        .arg(&target)
        .arg(MANIFEST)
        .output()
        .unwrap_or_else(|why| {
            panic!(
                "could not run glib-compile-resources ({why}). It comes with glib \
                 and is needed to build the icons into the program: install glib2 \
                 (Arch), glib2-devel (Fedora) or libglib2.0-dev (Debian, Ubuntu)."
            )
        });

    if !output.status.success() {
        panic!(
            "glib-compile-resources failed on {MANIFEST}:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
