use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use std::fmt::Write as _;

fn asset_files(directory: &str) -> io::Result<Vec<PathBuf>> {
    let path = Path::new(directory);
    println!("cargo:rerun-if-changed={directory}");

    let mut files = match fs::read_dir(path) {
        Ok(entries) => entries
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<Vec<_>>>()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    files.retain(|file| {
        file.is_file()
            && file
                .extension()
                .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("svg"))
    });
    files.sort();
    Ok(files)
}

fn append_asset_lookup(source: &mut String, function: &str, directory: &str) -> io::Result<()> {
    let asset_table = format!("{}_ASSETS", function.to_ascii_uppercase());
    let _ = writeln!(source, "static {asset_table}: &[(&str, &[u8])] = &[");

    for file in asset_files(directory)? {
        let Some(key) = file.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Some(file_name) = file.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let key = key.to_ascii_lowercase();
        let relative_path = format!("{directory}/{file_name}");
        let _ = writeln!(
            source,
            "    ({key:?}, include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/{relative_path}\"))),"
        );
    }

    source.push_str("];\n\n");
    let _ = writeln!(
        source,
        "#[must_use]\npub fn {function}(iso2_lower: &str) -> Option<&'static [u8]> {{\n    let index = {asset_table}.binary_search_by_key(&iso2_lower, |(key, _)| *key).ok()?;\n    Some({asset_table}[index].1)\n}}\n"
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=ui/app-window.slint");
    // ElementHandle needs compiler metadata for live scene inspection.
    let mut config = slint_build::CompilerConfiguration::new();
    if cfg!(debug_assertions) || env::var_os("CARGO_FEATURE_ANDROID_SMOKE").is_some() {
        config = config.with_debug_info(true);
    }
    slint_build::compile_with_config("ui/app-window.slint", config)?;
    let mut source = String::new();
    append_asset_lookup(&mut source, "flag_svg", "assets/flags")?;
    append_asset_lookup(&mut source, "map_svg", "assets/maps")?;

    let out_dir = env::var("OUT_DIR")?;
    fs::write(Path::new(&out_dir).join("assets.rs"), source)?;
    Ok(())
}
