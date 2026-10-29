//! `cargo xtask dist`: a release archive for this machine's platform.
//!
//! ```text
//! kgrust-<version>-<target>/
//!   bin/kg                          the toolchain
//!   bin/kanagawa                    the reference's command line, argv-compatible
//!   bin/libkanagawa-backend.*       the C++ backend `scripts/build-backend.sh` built,
//!                                   beside the executables, where they look for it
//!   share/kanagawa/library/         the standard library, from the submodule —
//!                                   `kg`'s import directory when nothing else names one
//!   share/kanagawa/runtime/rtl/     the runtime RTL generated designs build against,
//!                                   from the submodule, for `kg board`
//!   share/kanagawa/platform/        this project's devices, HALs and boards, their
//!                                   examples (`kg board list`, `kg board new`), the
//!                                   peripheral library and `kgbridge.py`
//!   LICENSE                         this project's
//!   THIRD-PARTY-NOTICES.md          everything else in the archive, with its license
//!   README.md
//! ```
//!
//! Then `<name>.tar.gz` and its `.sha256` beside the staged directory. The
//! notices are generated, not kept by hand: Kanagawa's own license for the
//! backend and the library; LLVM's, MLIR's and CIRCT's, linked statically
//! into the backend; Boost's, whose headers it compiles; and every Rust crate
//! `kg` and `kanagawa` link, from `cargo metadata`, each with the license
//! files its package ships.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

pub fn run(kanagawa_root: &Path, backend_dir: &Path, out: &Path, no_backend: bool) -> Result<()> {
    let workspace = crate::fresh::workspace();
    let version = env!("CARGO_PKG_VERSION");
    let target = host_target()?;
    let name = format!("kgrust-{version}-{target}");
    let stage = out.join(&name);
    let kanagawa_root = stage_tree(&workspace, kanagawa_root, backend_dir, no_backend, &stage)?;
    copy(&workspace.join("LICENSE"), &stage.join("LICENSE"))?;
    std::fs::write(stage.join("THIRD-PARTY-NOTICES.md"), notices(&workspace, &kanagawa_root, no_backend)?)?;
    std::fs::write(stage.join("README.md"), readme(&name, no_backend))?;

    let archive = out.join(format!("{name}.tar.gz"));
    let status = Command::new("tar").arg("-czf").arg(&archive).arg("-C").arg(out).arg(&name).status()?;
    if !status.success() {
        bail!("tar failed");
    }
    let sha = sha256(&archive)?;
    std::fs::write(out.join(format!("{name}.tar.gz.sha256")), format!("{sha}  {name}.tar.gz\n"))?;
    let size = std::fs::metadata(&archive)?.len();
    println!("{} ({:.1} MiB)\nsha256 {sha}", archive.display(), size as f64 / 1048576.0);
    Ok(())
}

/// Builds `kg` and `kanagawa` (the `ship` profile) and lays out `stage` as
/// an archive's tree minus its notices and README: `bin/`, then `share/kanagawa/`.
/// `cargo xtask install` stages the same tree, so what it installs is what a
/// release holds. Returns the canonical reference checkout.
fn stage_tree(workspace: &Path, kanagawa_root: &Path, backend_dir: &Path, no_backend: bool, stage: &Path) -> Result<PathBuf> {
    let kanagawa_root = kanagawa_root.canonicalize().with_context(|| {
        format!("no kanagawa checkout at {} — `git submodule update --init third_party/kanagawa`", kanagawa_root.display())
    })?;
    let lib_name = if cfg!(target_os = "macos") { "libkanagawa-backend.dylib" } else { "libkanagawa-backend.so" };
    let backend_lib = backend_dir.join(lib_name);
    if !no_backend && !backend_lib.is_file() {
        bail!(
            "no backend library at {} — run scripts/build-backend.sh first, or pass --no-backend for an archive that checks and edits but cannot build",
            backend_lib.display()
        );
    }

    eprintln!("dist: building kg and kanagawa (the `ship` profile)");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--profile", "ship", "-p", "kg-cli", "--bin", "kg", "--bin", "kanagawa"])
        .current_dir(workspace)
        .status()?;
    if !status.success() {
        bail!("cargo build failed");
    }

    let _ = std::fs::remove_dir_all(stage);
    std::fs::create_dir_all(stage.join("bin"))?;
    for bin in ["kg", "kanagawa"] {
        let exe = if cfg!(windows) { format!("{bin}.exe") } else { bin.to_owned() };
        copy(&workspace.join("target/ship").join(&exe), &stage.join("bin").join(&exe))?;
    }
    if !no_backend {
        copy(&backend_lib, &stage.join("bin").join(lib_name))?;
    }
    copy_tree(&kanagawa_root.join("library"), &stage.join("share/kanagawa/library"))?;
    copy_tree(&kanagawa_root.join("runtime/rtl"), &stage.join("share/kanagawa/runtime/rtl"))?;
    copy_tree(&workspace.join("platform"), &stage.join("share/kanagawa/platform"))?;
    Ok(kanagawa_root)
}

/// `cargo xtask install`: stage the release tree under `prefix` and link its
/// executables into `bin_dir`. `kg` finds the platform, the library and the
/// backend relative to its canonical path (`kg-config`'s `bundled_platform`,
/// `kg-backend-sys`'s `locate`), and canonicalising follows the links, so a
/// linked `kg` behaves as one run from an unpacked archive. `cargo install`
/// copies the binary alone, which is why it cannot.
pub fn install(kanagawa_root: &Path, backend_dir: &Path, prefix: &Path, bin_dir: &Path, no_backend: bool) -> Result<()> {
    let workspace = crate::fresh::workspace();
    let bins: Vec<String> = ["kg", "kanagawa"].iter().map(|b| if cfg!(windows) { format!("{b}.exe") } else { (*b).to_owned() }).collect();
    // A real file in the way is a `cargo install`'s: refuse before building,
    // rather than clobber a file this command did not make.
    for bin in &bins {
        let link = bin_dir.join(bin);
        if link.symlink_metadata().is_ok_and(|m| !m.file_type().is_symlink()) {
            bail!("{} is a regular file, not a link — remove it first (`cargo uninstall kg-cli` if cargo put it there)", link.display());
        }
    }
    stage_tree(&workspace, kanagawa_root, backend_dir, no_backend, prefix)?;
    std::fs::create_dir_all(bin_dir)?;
    for bin in &bins {
        let link = bin_dir.join(bin);
        let _ = std::fs::remove_file(&link);
        link_to(&prefix.join("bin").join(bin), &link)?;
        println!("{} -> {}", link.display(), prefix.join("bin").join(bin).display());
    }
    println!("installed to {}; put {} on your PATH if it is not", prefix.display(), bin_dir.display());
    Ok(())
}

#[cfg(unix)]
fn link_to(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).with_context(|| format!("linking {}", link.display()))
}

#[cfg(windows)]
fn link_to(target: &Path, link: &Path) -> Result<()> {
    std::os::windows::fs::symlink_file(target, link).with_context(|| format!("linking {}", link.display()))
}

/// `rustc -vV`'s host triple: the platform the binaries were built for.
fn host_target() -> Result<String> {
    let output = Command::new("rustc").arg("-vV").output()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_owned))
        .context("rustc -vV printed no host")
}

fn copy(from: &Path, to: &Path) -> Result<()> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::copy(from, to).with_context(|| format!("copying {} to {}", from.display(), to.display()))?;
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            // A board example's own builds (`kg board`), and the bytecode
            // Python leaves beside a test it imported, stay out.
            if entry.file_name() == "target" || entry.file_name() == "__pycache__" {
                continue;
            }
            copy_tree(&path, &target)?;
        } else {
            copy(&path, &target)?;
        }
    }
    Ok(())
}

fn sha256(path: &Path) -> Result<String> {
    let output = if cfg!(target_os = "macos") {
        Command::new("shasum").args(["-a", "256"]).arg(path).output()?
    } else {
        Command::new("sha256sum").arg(path).output()?
    };
    String::from_utf8_lossy(&output.stdout).split_whitespace().next().map(str::to_owned).context("no checksum printed")
}

/// THIRD-PARTY-NOTICES.md: what the archive holds besides this project's own
/// code, each with its license text.
fn notices(workspace: &Path, kanagawa_root: &Path, no_backend: bool) -> Result<String> {
    let mut out = String::from(
        "# Third-party notices\n\n\
         This archive contains software beyond kgrust's own (see `LICENSE`). Each part is listed with its \
         license, followed by the license texts.\n\n",
    );
    let read = |p: PathBuf| std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()));
    out.push_str("## Kanagawa\n\n");
    out.push_str(
        "The standard library (`share/kanagawa/library`), the runtime RTL \
         (`share/kanagawa/runtime/rtl`) and the code generator (`bin/libkanagawa-backend.*`) are built from [microsoft/kanagawa](https://github.com/microsoft/kanagawa), \
         MIT License.\n\n```text\n",
    );
    out.push_str(read(kanagawa_root.join("LICENSE"))?.trim_end());
    out.push_str("\n```\n\n");
    if !no_backend {
        let circt = kanagawa_root.join("thirdparty/circt");
        out.push_str("## LLVM, MLIR and CIRCT\n\n");
        out.push_str(
            "Linked statically into `bin/libkanagawa-backend.*`: [LLVM and MLIR](https://llvm.org) and \
             [CIRCT](https://circt.llvm.org), Apache License 2.0 with LLVM Exceptions.\n\n```text\n",
        );
        out.push_str(read(circt.join("llvm/llvm/LICENSE.TXT"))?.trim_end());
        out.push_str("\n```\n\nCIRCT:\n\n```text\n");
        out.push_str(read(circt.join("LICENSE"))?.trim_end());
        out.push_str("\n```\n\n## Boost\n\nHeaders compiled into `bin/libkanagawa-backend.*`, Boost Software License 1.0.\n\n```text\n");
        out.push_str(BOOST_LICENSE.trim_end());
        out.push_str("\n```\n\n");
    }

    out.push_str("## Rust crates\n\nLinked into `bin/kg` and `bin/kanagawa`.\n\n");
    let crates = linked_crates(workspace)?;
    out.push_str("| Crate | Version | License |\n|---|---|---|\n");
    for c in &crates {
        out.push_str(&format!("| {} | {} | {} |\n", c.name, c.version, c.license));
    }
    out.push('\n');
    for c in &crates {
        if c.texts.is_empty() {
            continue;
        }
        out.push_str(&format!("### {} {}\n\n", c.name, c.version));
        for (file, text) in &c.texts {
            out.push_str(&format!("`{file}`:\n\n```text\n{}\n```\n\n", text.trim_end()));
        }
    }
    Ok(out)
}

struct Crate {
    name: String,
    version: String,
    license: String,
    /// The license and notice files its package ships, by name.
    texts: Vec<(String, String)>,
}

/// Every non-workspace package `kg-cli`'s binaries depend on, transitively,
/// as `cargo metadata` resolves them for this platform.
fn linked_crates(workspace: &Path) -> Result<Vec<Crate>> {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--locked", "--filter-platform", &host_target()?])
        .current_dir(workspace)
        .output()?;
    if !output.status.success() {
        bail!("cargo metadata failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    let meta: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let packages: BTreeMap<&str, &serde_json::Value> = meta["packages"]
        .as_array()
        .context("no packages")?
        .iter()
        .filter_map(|p| Some((p["id"].as_str()?, p)))
        .collect();
    let members: Vec<&str> = meta["workspace_members"].as_array().context("no members")?.iter().filter_map(|m| m.as_str()).collect();
    let nodes: BTreeMap<&str, &serde_json::Value> = meta["resolve"]["nodes"]
        .as_array()
        .context("no resolve graph")?
        .iter()
        .filter_map(|n| Some((n["id"].as_str()?, n)))
        .collect();
    let root = packages.iter().find(|(_, p)| p["name"] == "kg-cli").map(|(id, _)| *id).context("no kg-cli package")?;

    // Normal dependencies only: build scripts' and proc macros' run at
    // build time and are not linked.
    let mut seen = std::collections::BTreeSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        for dep in nodes.get(id).and_then(|n| n["deps"].as_array()).into_iter().flatten() {
            let normal = dep["dep_kinds"].as_array().is_some_and(|kinds| kinds.iter().any(|k| k["kind"].is_null()));
            if normal && let Some(pkg) = dep["pkg"].as_str() {
                let proc_macro = packages.get(pkg).and_then(|p| p["targets"].as_array()).is_some_and(|targets| {
                    targets.iter().all(|t| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "proc-macro")))
                });
                if !proc_macro {
                    stack.push(pkg);
                }
            }
        }
    }
    let mut crates: Vec<Crate> = seen
        .into_iter()
        .filter(|id| !members.contains(id))
        .filter_map(|id| packages.get(id))
        .map(|p| {
            let dir = Path::new(p["manifest_path"].as_str().unwrap_or("")).parent().map(Path::to_owned).unwrap_or_default();
            let mut texts: Vec<(String, String)> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| {
                    let file = e.file_name().to_string_lossy().into_owned();
                    let upper = file.to_ascii_uppercase();
                    let wanted = ["LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT"].iter().any(|w| upper.starts_with(w));
                    (wanted && e.path().is_file()).then(|| Some((file, std::fs::read_to_string(e.path()).ok()?)))?
                })
                .collect();
            texts.sort();
            Crate {
                name: p["name"].as_str().unwrap_or("").to_owned(),
                version: p["version"].as_str().unwrap_or("").to_owned(),
                license: p["license"].as_str().unwrap_or("see its files").to_owned(),
                texts,
            }
        })
        .collect();
    crates.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    Ok(crates)
}

fn readme(name: &str, no_backend: bool) -> String {
    let build = if no_backend {
        "This archive has no code generator: `kg check`, `kg fmt`, `kg lsp` and `kg doc` work; `kg build` needs \
         `libkanagawa-backend` (`KG_BACKEND_DIR`).\n"
    } else {
        "`kg build` compiles to SystemVerilog through the bundled `libkanagawa-backend`.\n"
    };
    format!(
        "# {name}\n\n\
         The kgrust toolchain for [Kanagawa](https://github.com/microsoft/kanagawa).\n\n\
         Put `bin` on your `PATH`:\n\n\
         ```sh\nexport PATH=\"$PWD/bin:$PATH\"\nkg check my_design.k\n```\n\n\
         The standard library ships in `share/kanagawa/library` and is used when a project names no import \
         directory of its own. {build}\n\
         Boards: `kg board list` lists them, and `kg board new hello --board virtual --example accel` starts a \
         project that runs with no hardware (`kg board serve` opens its front panel); the guide is \
         `share/kanagawa/platform/README.md`.\n\n\
         `kg --help` lists the commands; `kanagawa` takes the reference compiler's command line unchanged.\n\n\
         Licenses: `LICENSE` (kgrust, MIT) and `THIRD-PARTY-NOTICES.md` (everything else in this archive).\n"
    )
}

const BOOST_LICENSE: &str = "Boost Software License - Version 1.0 - August 17th, 2003

Permission is hereby granted, free of charge, to any person or organization
obtaining a copy of the software and accompanying documentation covered by
this license (the \"Software\") to use, reproduce, display, distribute,
execute, and transmit the Software, and to prepare derivative works of the
Software, and to permit third-parties to whom the Software is furnished to
do so, all subject to the following:

The copyright notices in the Software and this entire statement, including
the above license grant, this restriction and the following disclaimer,
must be included in all copies of the Software, in whole or in part, and
all derivative works of the Software, unless such copies or derivative
works are solely in the form of machine-executable object code generated by
a source language processor.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE, TITLE AND NON-INFRINGEMENT. IN NO EVENT
SHALL THE COPYRIGHT HOLDERS OR ANYONE DISTRIBUTING THE SOFTWARE BE LIABLE
FOR ANY DAMAGES OR OTHER LIABILITY, WHETHER IN CONTRACT, TORT OR OTHERWISE,
ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
";
