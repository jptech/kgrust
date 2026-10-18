//! AMD Vivado, in batch mode, through the platform's Tcl
//! (`vendor/vivado/build.tcl`, `program.tcl`). Untested: written without
//! Vivado to hand (`docs/plans/platform-layer.md` §6).

use std::io::Write;
use std::process::Command;

use camino::Utf8PathBuf;

use crate::{Design, Failure, Result, require};

/// Synthesis, implementation, reports and the bitstream, from the bundle.
/// Without Vivado, says how to run the bundle where it is.
pub fn bitstream(design: &Design, log: &mut dyn Write) -> Result<Utf8PathBuf> {
    let flow = design.board.platform.join("vendor/vivado/build.tcl");
    if require("vivado").is_err() {
        return Err(Failure::Usage(format!(
            "vivado is not on PATH; the bundle is ready. On a machine with it:\n  \
             vivado -mode batch -nojournal -source {flow} -tclargs {}",
            design.out
        )));
    }
    let status = Command::new("vivado")
        .args(["-mode", "batch", "-nojournal", "-log", "vivado.log", "-source"])
        .arg(&flow)
        .arg("-tclargs")
        .arg(&design.out)
        .current_dir(&design.out)
        .status()
        .map_err(|e| Failure::Usage(format!("cannot run vivado: {e}")))?;
    if !status.success() {
        return Err(Failure::Failed(format!("vivado failed ({}/vivado.log)", design.out)));
    }
    let bit = design.out.join("vivado").join(format!("{}.bit", design.top()?));
    let _ = writeln!(log, "bitstream: {bit}");
    Ok(bit)
}

/// The last bitstream into the board over JTAG.
pub fn program(design: &Design, log: &mut dyn Write) -> Result<()> {
    require("vivado")?;
    let bit = design.out.join("vivado").join(format!("{}.bit", design.top()?));
    if !bit.is_file() {
        return Err(Failure::Usage("no bitstream; run `kg board bitstream` first".to_owned()));
    }
    let _ = writeln!(log, "flash: {bit}");
    let status = Command::new("vivado")
        .args(["-mode", "batch", "-nojournal", "-log"])
        .arg(design.out.join("program.log"))
        .arg("-source")
        .arg(design.board.platform.join("vendor/vivado/program.tcl"))
        .arg("-tclargs")
        .arg(&bit)
        .status()
        .map_err(|e| Failure::Usage(format!("cannot run vivado: {e}")))?;
    if status.success() { Ok(()) } else { Err(Failure::Failed(format!("vivado failed ({}/program.log)", design.out))) }
}
