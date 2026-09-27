// Embeds the application manifest and the VERSIONINFO resource with the MSVC linker
// (no resource compiler needed: the .res file is written here, and link.exe accepts it).
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

    // The installer reads the installed copy's version from this resource (ARCHITECTURE §7.1).
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let res = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("version.res");
    std::fs::write(&res, version_res(&version)).unwrap();
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

fn pad(b: &mut Vec<u8>) {
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

/// One version-resource node: wLength, wValueLength, wType, key, value, children.
fn node(key: &str, value: &[u8], value_len: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend(utf16z(key));
    pad(&mut b);
    b.extend(value);
    for c in children {
        pad(&mut b);
        b.extend(c);
    }
    let len = b.len() as u16;
    b[0..2].copy_from_slice(&len.to_le_bytes());
    b[2..4].copy_from_slice(&value_len.to_le_bytes());
    b[4..6].copy_from_slice(&u16::from(text).to_le_bytes());
    b
}

fn string(key: &str, value: &str) -> Vec<u8> {
    let v = utf16z(value);
    node(key, &v, (v.len() / 2) as u16, true, &[])
}

fn version_res(version: &str) -> Vec<u8> {
    let mut n = version.split(['-', '+']).next().unwrap().split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let (major, minor, patch) = (n.next().unwrap_or(0), n.next().unwrap_or(0), n.next().unwrap_or(0));
    let ms = (major << 16) | minor;
    let ls = patch << 16;
    let fixed: Vec<u8> = [
        0xFEEF_04BDu32, // signature
        0x0001_0000,    // struct version
        ms,
        ls, // file version
        ms,
        ls,          // product version
        0x3F,        // flags mask
        0,           // flags
        0x0004_0004, // VOS_NT_WINDOWS32
        1,           // VFT_APP
        0,
        0,
        0, // subtype, date
    ]
    .iter()
    .flat_map(|d| d.to_le_bytes())
    .collect();
    let strings = node(
        "040904B0",
        &[],
        0,
        true,
        &[
            string("CompanyName", "MeltAlarm project"),
            string("FileDescription", "MeltAlarm: 12V-2x6 GPU cable monitor"),
            string("FileVersion", version),
            string("InternalName", "meltalarm"),
            string("LegalCopyright", "MIT License"),
            string("OriginalFilename", "meltalarm.exe"),
            string("ProductName", "MeltAlarm"),
            string("ProductVersion", version),
        ],
    );
    let string_info = node("StringFileInfo", &[], 0, true, &[strings]);
    let translation = node("Translation", &0x04B0_0409u32.to_le_bytes(), 4, false, &[]);
    let var_info = node("VarFileInfo", &[], 0, true, &[translation]);
    let mut data = node("VS_VERSION_INFO", &fixed, fixed.len() as u16, false, &[string_info, var_info]);
    pad(&mut data);

    // .res file: an empty header entry, then RT_VERSION (16) #1, language en-US.
    let header = |data_size: u32, ty: u16, name: u16, flags: u16, lang: u16| {
        let mut h = Vec::new();
        h.extend(data_size.to_le_bytes());
        h.extend(32u32.to_le_bytes());
        h.extend([0xFF, 0xFF]);
        h.extend(ty.to_le_bytes());
        h.extend([0xFF, 0xFF]);
        h.extend(name.to_le_bytes());
        h.extend(0u32.to_le_bytes()); // data version
        h.extend(flags.to_le_bytes());
        h.extend(lang.to_le_bytes());
        h.extend(0u32.to_le_bytes()); // version
        h.extend(0u32.to_le_bytes()); // characteristics
        h
    };
    let mut res = header(0, 0, 0, 0, 0);
    res.extend(header(data.len() as u32, 16, 1, 0x0030, 0x0409));
    res.extend(data);
    res
}
