//! Compresses the spell-check dictionaries in `dictionaries/` so they can be
//! embedded in the binary. `spell` inflates a language the first time it is
//! used, which keeps the embedded size small and costs nothing for languages
//! that are never enabled.

use flate2::{write::DeflateEncoder, Compression};
use std::{
    env,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let source = manifest.join("dictionaries");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("dictionaries");
    fs::create_dir_all(&out).expect("create the dictionary output directory");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=dictionaries");

    // Release builds are the only ones that ship, so only they pay for the
    // slow, smallest setting. Compressing 19 MB at level 9 in an unoptimised
    // build script would dominate every clean debug or test build.
    let level = if env::var("PROFILE").as_deref() == Ok("release") {
        Compression::best()
    } else {
        Compression::fast()
    };

    let mut languages: Vec<_> = fs::read_dir(&source)
        .expect("read core/dictionaries")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .collect();
    languages.sort_by_key(|entry| entry.file_name());

    for entry in languages {
        let code = entry.file_name().to_string_lossy().into_owned();
        for extension in ["aff", "dic"] {
            let input = entry.path().join(format!("index.{extension}"));
            compress(&input, &out.join(format!("{code}.{extension}")), level);
        }
    }
}

fn compress(input: &Path, output: &Path, level: Compression) {
    let text =
        fs::read_to_string(input).unwrap_or_else(|err| panic!("read {}: {err}", input.display()));
    // A byte-order mark before `SET UTF-8` is not part of the dictionary.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let file =
        File::create(output).unwrap_or_else(|err| panic!("create {}: {err}", output.display()));
    let mut encoder = DeflateEncoder::new(file, level);
    encoder
        .write_all(text.as_bytes())
        .and_then(|()| encoder.finish().map(|_| ()))
        .unwrap_or_else(|err| panic!("compress {}: {err}", input.display()));
}
