//! `abi.md` §2.2 and §3, pinned: the sizes and offsets the spec measured by
//! compiling the real headers, asserted against what bindgen produced. A
//! failure here means the reference's `options.h` moved and every fixture
//! recorded under the old layout is suspect.

use std::mem::{offset_of, size_of};

use kg_backend_sys::raw::{CodeGenOptions, Location, Options, PlacementOptions};

#[test]
fn sizes() {
    assert_eq!(size_of::<PlacementOptions>(), 40, "§3.1");
    assert_eq!(size_of::<CodeGenOptions>(), 448, "§3.2");
    assert_eq!(size_of::<Options>(), 520, "§3.3");
    assert_eq!(size_of::<Location>(), 48, "§2.2: five size_t and a bool, padded");
}

#[test]
fn codegen_offsets_around_every_padding_run() {
    // §3.2: the field after each of the seven padding runs.
    assert_eq!(offset_of!(CodeGenOptions, _semaphoreDelay), 88);
    assert_eq!(offset_of!(CodeGenOptions, _codeCoverageMuxThreshold), 104);
    assert_eq!(offset_of!(CodeGenOptions, _maxBlockRam), 120);
    assert_eq!(offset_of!(CodeGenOptions, _fifoToDspThreshold), 344);
    assert_eq!(offset_of!(CodeGenOptions, _verbosity), 352);
    assert_eq!(offset_of!(CodeGenOptions, _logWorkListSize), 392);
    assert_eq!(offset_of!(CodeGenOptions, _warningsAsErrors), 440);
}

#[test]
fn options_offsets() {
    assert_eq!(offset_of!(Options, _frequency), 0);
    assert_eq!(offset_of!(Options, _deviceName), 8);
    assert_eq!(offset_of!(Options, _cmdArgs), 16);
    assert_eq!(offset_of!(Options, _codegenOptions), 24);
    assert_eq!(offset_of!(Options, _placementOptions), 472);
    assert_eq!(offset_of!(Options, _fileNames), 512);
    assert_eq!(offset_of!(Location, _fileIndex), 32);
    assert_eq!(offset_of!(Location, _valid), 40);
}
