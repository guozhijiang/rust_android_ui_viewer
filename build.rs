use std::{env, path::PathBuf, process::Command};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let icon = manifest_dir.join("assets/appicon.ico");
    let rc = manifest_dir.join("assets/appicon.rc");
    let res = out_dir.join("appicon.res");

    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed={}", rc.display());

    let rc_exe = find_rc_exe().expect(
        "rc.exe not found. Install the Windows SDK or set the RC environment variable.",
    );

    let status = Command::new(&rc_exe)
        .arg("/fo")
        .arg(res.to_str().unwrap())
        .arg(rc.to_str().unwrap())
        .status()
        .unwrap_or_else(|e| panic!("failed to run {}: {}", rc_exe.display(), e));

    if !status.success() {
        panic!("rc.exe failed to compile {}", rc.display());
    }

    println!("cargo:rustc-link-arg-bins={}", res.display());
}

fn find_rc_exe() -> Option<PathBuf> {
    if let Ok(rc) = env::var("RC") {
        let p = PathBuf::from(rc);
        if p.exists() {
            return Some(p);
        }
    }

    let host = env::var("HOST").unwrap_or_else(|_| "x86_64-pc-windows-msvc".to_string());
    let arch_dir = if host.contains("i686") {
        "x86"
    } else {
        "x64"
    };

    let kits_root = PathBuf::from("C:\\Program Files (x86)\\Windows Kits\\10\\bin");
    if kits_root.exists() {
        let mut entries: Vec<_> = std::fs::read_dir(&kits_root)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        entries.sort();
        for ver in entries.iter().rev() {
            let candidate = ver.join(arch_dir).join("rc.exe");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    None
}
