// Embeds the application manifest, the VERSIONINFO resource and the app icon with the MSVC
// linker (no resource compiler needed: the .res file is written here, and link.exe accepts it).
#[path = "src/paint.rs"]
mod paint;

/// docs/DESIGN.md "App icon": drawn per size.
const ICON_SIZES: [i32; 9] = [16, 20, 24, 32, 40, 48, 64, 96, 128];

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
    println!("cargo:rerun-if-changed=src/paint.rs");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let res = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("meltalarm.res");
    std::fs::write(&res, resources(&version)).unwrap();
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

/// A .res file: an empty header entry, then each resource (type, ordinal id, en-US).
fn resources(version: &str) -> Vec<u8> {
    const RT_ICON: u16 = 3;
    const RT_GROUP_ICON: u16 = 14;
    const RT_VERSION: u16 = 16;
    let mut res = res_entry(0, 0, &[]);
    let mut group = Vec::new();
    group.extend([0u8, 0, 1, 0]); // reserved, type = icon
    group.extend((ICON_SIZES.len() as u16).to_le_bytes());
    for (i, &size) in ICON_SIZES.iter().enumerate() {
        let dib = icon_dib(size);
        let id = i as u16 + 1;
        group.extend([size as u8, size as u8, 0, 0]); // width, height, colors, reserved
        group.extend(1u16.to_le_bytes()); // planes
        group.extend(32u16.to_le_bytes()); // bits
        group.extend((dib.len() as u32).to_le_bytes());
        group.extend(id.to_le_bytes());
        res.extend(res_entry(RT_ICON, id, &dib));
    }
    // Group #1: the lowest id, so Explorer uses it as the exe's icon.
    res.extend(res_entry(RT_GROUP_ICON, 1, &group));
    res.extend(res_entry(RT_VERSION, 1, &version_info(version)));
    res
}

fn res_entry(ty: u16, name: u16, data: &[u8]) -> Vec<u8> {
    let mut h = Vec::new();
    h.extend((data.len() as u32).to_le_bytes());
    h.extend(32u32.to_le_bytes()); // header size
    h.extend([0xFF, 0xFF]);
    h.extend(ty.to_le_bytes());
    h.extend([0xFF, 0xFF]);
    h.extend(name.to_le_bytes());
    h.extend(0u32.to_le_bytes()); // data version
    h.extend(if ty == 0 { 0u16 } else { 0x1030 }.to_le_bytes()); // moveable, pure
    h.extend(if ty == 0 { 0u16 } else { 0x0409 }.to_le_bytes()); // en-US
    h.extend(0u32.to_le_bytes()); // version
    h.extend(0u32.to_le_bytes()); // characteristics
    h.extend(data);
    pad(&mut h);
    h
}

/// 32-bit DIB with alpha, bottom-up, plus an all-zero AND mask (as in .ico files).
fn icon_dib(size: i32) -> Vec<u8> {
    let argb = paint::app_icon(size);
    let s = size as usize;
    let mask_row = s.div_ceil(32) * 4;
    let mut b = Vec::new();
    for v in [40u32, size as u32, 2 * size as u32] {
        b.extend(v.to_le_bytes()); // biSize, biWidth, biHeight (color + mask)
    }
    b.extend(1u16.to_le_bytes()); // planes
    b.extend(32u16.to_le_bytes()); // bit count
    b.extend(0u32.to_le_bytes()); // BI_RGB
    b.extend(((s * s * 4 + mask_row * s) as u32).to_le_bytes());
    b.extend([0u8; 16]); // resolution, colors
    for row in (0..s).rev() {
        for px in &argb[row * s..(row + 1) * s] {
            b.extend(px.to_le_bytes()); // B, G, R, A
        }
    }
    b.extend(vec![0u8; mask_row * s]);
    b
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

fn version_info(version: &str) -> Vec<u8> {
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
            string("FileDescription", "MeltAlarm"),
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
    node("VS_VERSION_INFO", &fixed, fixed.len() as u16, false, &[string_info, var_info])
}
