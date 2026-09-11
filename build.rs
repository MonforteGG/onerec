use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/onerec.ico");
    println!("cargo:rerun-if-env-changed=ONEREC_RC");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let icon = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
        .join("assets/onerec.ico");
    let resource_script = out_dir.join("onerec.rc");
    let resource = out_dir.join("onerec.res");
    fs::write(
        &resource_script,
        format!(
            "1 ICON \"{}\"\n",
            icon.display().to_string().replace('\\', "/")
        ),
    )
    .expect("writing the icon resource script");
    let status = Command::new(resource_compiler())
        .args(["/nologo", "/c65001", "/fo"])
        .arg(&resource)
        .arg(&resource_script)
        .status()
        .expect("running Windows SDK rc.exe (or set ONEREC_RC to its path)");
    assert!(status.success(), "compiling the application icon failed");
    println!("cargo:rustc-link-arg-bins={}", resource.display());
    let manifest = out_dir.join("onerec.manifest");
    fs::write(&manifest, MANIFEST).expect("writing the application manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );
}

// A normal Developer Command Prompt already has rc.exe on PATH. Also support
// cargo from a plain shell with the Windows SDK installed, without a new crate.
fn resource_compiler() -> PathBuf {
    if let Some(path) = env::var_os("ONEREC_RC") {
        return path.into();
    }
    if let Some(paths) = env::var_os("PATH") {
        for path in env::split_paths(&paths) {
            let candidate = path.join("rc.exe");
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    if let Some(program_files) = env::var_os("ProgramFiles(x86)") {
        let sdk = PathBuf::from(program_files).join("Windows Kits/10/bin");
        let mut versions: Vec<_> = fs::read_dir(sdk)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        versions.sort();
        for version in versions.into_iter().rev() {
            for host in ["x64", "x86", "arm64"] {
                let candidate = version.join(host).join("rc.exe");
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }
    PathBuf::from("rc.exe")
}

// Without the v6 common controls dependency the combo boxes get Windows 95 chrome.
// Controls and fonts are laid out in logical units and rebuilt on WM_DPICHANGED.
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="onerec" version="1.0.0.0" processorArchitecture="*"/>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2, PerMonitor</dpiAwareness>
    </windowsSettings>
  </application>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#;
