use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

/// PDFium build matching the `pdfium_8125` feature of pdfium-render.
const PDFIUM_RELEASE: &str = "chromium/8125";

fn main() {
    slint_build::compile("ui/app.slint").expect("failed to compile ui/app.slint");
    println!("cargo:rerun-if-changed=build.rs");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if let Err(err) = provide_pdfium() {
            println!("cargo:warning=could not provide pdfium.dll: {err}");
        }
    }
}

/// Downloads the prebuilt PDFium DLL once (into `target/pdfium/`) and copies
/// it next to the built executable. Uses the curl and tar that ship with Windows.
fn provide_pdfium() -> Result<(), String> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").map_err(|e| e.to_string())?);
    // OUT_DIR is target/<profile>/build/<pkg>/out
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .ok_or("unexpected OUT_DIR layout")?;
    let target_dir = profile_dir.parent().ok_or("unexpected OUT_DIR layout")?;

    let cache_dir = target_dir
        .join("pdfium")
        .join(PDFIUM_RELEASE.replace('/', "-"));
    let dll = cache_dir.join("bin").join("pdfium.dll");
    if !dll.exists() {
        download(&cache_dir)?;
    }
    let dest = profile_dir.join("pdfium.dll");
    // Skip when already in place: the DLL is locked while dpdf is running.
    let size = |p: &Path| fs::metadata(p).map(|m| m.len()).ok();
    if size(&dest).is_none() || size(&dest) != size(&dll) {
        fs::copy(&dll, dest).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn download(cache_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
    let archive = cache_dir.join("pdfium-win-x64.tgz");
    let url = format!(
        "https://github.com/bblanchon/pdfium-binaries/releases/download/{PDFIUM_RELEASE}/pdfium-win-x64.tgz"
    );
    run(Command::new("curl")
        .args(["-sSfL", "-o"])
        .arg(&archive)
        .arg(&url))?;
    run(Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(cache_dir))?;
    fs::remove_file(&archive).map_err(|e| e.to_string())
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let status = cmd.status().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{cmd:?} failed with {status}"))
    }
}
