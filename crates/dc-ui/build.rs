//! #246 i18n: generate the typed Fluent accessors (`src/l10n.rs`,
//! committed) from the project-level `locales/<lang>/*.ftl` catalogs.
//! Argument types are read from the `en` reference locale's type
//! comments; `de` must carry the same key/variable set or this build
//! fails — the same contract `flynt` checks on the catalogs.

fn main() -> std::process::ExitCode {
    // Watch the whole tree: adding a locale folder is itself a
    // change that must regenerate the accessors.
    println!("cargo:rerun-if-changed=../../locales");
    // Bidi isolation ON (#257 item 3): Persian ships, and interpolated
    // values (file paths, identifiers, page names) keep their run
    // direction inside RTL sentences. The marks are zero-width and
    // render nothing; the group_edit dedupe-index tests that keyed on
    // the unwrapped forms quote the wrapped forms now.
    if let Err(err) = windows_version_resource() {
        eprintln!("tarahi.exe version resource: {err}");
        return std::process::ExitCode::FAILURE;
    }
    let opts = fluent_typed::BuildOptions::default()
        .with_locales_folder("../../locales")
        .with_default_language("en");
    fluent_typed::build_from_locales_folder(opts)
}

/// The release version: `TARAHI_VERSION` when the release workflow sets
/// it from the tag, else the crate version.
fn release_version() -> String {
    println!("cargo:rerun-if-env-changed=TARAHI_VERSION");
    std::env::var("TARAHI_VERSION")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned())
}

/// Embeds a VERSIONINFO resource in `tarahi.exe` whose ProductVersion and
/// FileVersion are the release version. Windows reads them for the file's
/// properties, and the ArtCraft Launcher compares ProductVersion with the
/// release it installed (it rolls back a portable install whose exe has
/// none). Other targets build without it.
#[cfg(windows)]
fn windows_version_resource() -> Result<(), String> {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return Ok(());
    }
    let version = release_version();
    let mut parts = version
        .split(|c: char| !c.is_ascii_digit())
        .map(|p| p.parse::<u64>().unwrap_or(0).min(0xFFFF));
    let mut packed = 0u64;
    for shift in [48, 32, 16] {
        packed |= parts.next().unwrap_or(0) << shift;
    }
    let mut res = winresource::WindowsResource::new();
    res.set("ProductName", "Tarahi")
        .set("FileDescription", "Tarahi")
        .set("ProductVersion", &version)
        .set("FileVersion", &version)
        .set_version_info(winresource::VersionInfo::PRODUCTVERSION, packed)
        .set_version_info(winresource::VersionInfo::FILEVERSION, packed);
    res.compile().map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn windows_version_resource() -> Result<(), String> {
    // Read the variable anyway, so a change to it reruns the script on
    // every host and the release step is easy to follow.
    let _ = release_version();
    Ok(())
}
