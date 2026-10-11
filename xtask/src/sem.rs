//! `cargo xtask infer-identity`: incremental inference against the whole
//! walk, every root, trajectories included.
//!
//! Until sprint W3.7 this module was also `cargo xtask sem`, the shadow gate
//! of `docs/history/ir-elaboration.md` step M1: the IR engine's types against
//! the CST inference engine's, node for node, the reference adjudicating
//! disagreements. It went with that engine.

use std::path::Path;

use anyhow::Result;

/// `cargo xtask infer-identity`: every root elaborated with inference walked
/// whole and incrementally — the incremental run the editor's, layered,
/// its first answer graded against the loop bounded where it was taken
/// (`kg_db::looped::infer_identity`, sprint W7.6); red on any root whose
/// results differ.
pub fn identity(kanagawa_root: &Path, filter: Option<&str>, dump: Option<&Path>) -> Result<()> {
    // Canonical, as the settings' import paths are: a root collected under
    // `../kanagawa` is another file id than the same file reached by an
    // import, and a library root that imports what imports it was loaded
    // twice (found 2026-09-27 diffing two builds' dumps).
    let kanagawa_root = &kanagawa_root.canonicalize()?;
    let started = std::time::Instant::now();
    if let Some(dir) = dump {
        std::fs::create_dir_all(dir)?;
    }
    let mut files = Vec::new();
    for dir in ["library", "test"] {
        crate::corpus::collect(&kanagawa_root.join(dir), &mut files);
    }
    files.retain(|f| !f.components().any(|c| c.as_os_str() == "syntax"));
    if let Some(f) = filter {
        files.retain(|p| p.to_string_lossy().contains(f));
    }
    files.sort();
    // Two elaborations of a program are alive at once: fewer workers than
    // the other corpus gates (`crate::bounded`'s measurements).
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(6);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let slots: Vec<std::sync::Mutex<Option<Option<kg_db::looped::Identity>>>> =
        files.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut db = kg_db::Db::new(crate::bench::rooted_settings(kanagawa_root));
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let Some(path) = files.get(i) else { break };
                    let Ok(text) = std::fs::read_to_string(path) else { continue };
                    let Ok(utf8) = camino::Utf8PathBuf::from_path_buf(path.clone()) else { continue };
                    let root = db.set_file_text(&utf8, text);
                    if let Some(dir) = dump {
                        let name = path.strip_prefix(kanagawa_root).unwrap_or(path).to_string_lossy().replace('/', "__");
                        let _ = std::fs::write(dir.join(name), kg_db::looped::fingerprint(&db, &[root]));
                        *slots[i].lock().expect("no panics hold this lock") = Some(None);
                        continue;
                    }
                    let verdict = kg_db::looped::infer_identity(&db, &[root]);
                    *slots[i].lock().expect("no panics hold this lock") = Some(Some(verdict));
                }
            });
        }
    });
    let mut differ = Vec::new();
    let mut graded = 0usize;
    let mut firsts = 0usize;
    for (path, slot) in files.iter().zip(slots) {
        let Some(verdict) = slot.into_inner().expect("threads joined") else { continue };
        graded += 1;
        let Some(verdict) = verdict else { continue };
        firsts += usize::from(verdict.first_answer);
        if let Some(d) = verdict.differ {
            differ.push(format!("  DIFFER {}: {d}", path.strip_prefix(kanagawa_root).unwrap_or(path).display()));
        }
    }
    // One line, last: `scripts/identity.sh` reads it with `tail -1`.
    let firsts = if dump.is_some() { String::new() } else { format!("; the editor's first answer graded on {firsts}") };
    println!(
        "infer-identity: {graded} roots in {:.1}s — {} identical, {} differ{firsts}",
        started.elapsed().as_secs_f64(),
        graded - differ.len(),
        differ.len()
    );
    for d in &differ {
        println!("{d}");
    }
    if !differ.is_empty() {
        anyhow::bail!("{} roots elaborate differently with incremental inference", differ.len());
    }
    Ok(())
}
