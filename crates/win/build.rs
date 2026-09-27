// Embeds the application manifest with the MSVC linker (no resource compiler needed).
fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let simulate = std::env::var_os("CARGO_FEATURE_SIMULATE").is_some();
    // Real builds must run elevated: the MSI PSU coexistence mutex is admin-only (spec F3).
    // Simulated builds never touch USB and run as the invoking user.
    let level = if simulate { "asInvoker" } else { "requireAdministrator" };
    println!("cargo:rerun-if-changed=meltalarm.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    let manifest = std::path::Path::new(&dir).join("meltalarm.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
    println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:level='{level}' uiAccess='false'");
}
