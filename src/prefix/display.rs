//! Display scaling for Office: the DPI Wine reports to Windows programs.
//!
//! Office draws its UI at this DPI, so a higher value gives larger, sharp text on
//! high-resolution screens. It is read when a Wine program starts, so a change applies the
//! next time Office starts. `winecfg` (Graphics → Screen resolution) writes the same values.

use super::wine::RegData;

/// The offered scales as (percent, DPI). 96 DPI is Windows' 100%.
pub const SCALES: &[(u32, u32)] = &[(100, 96), (125, 120), (150, 144), (175, 168), (200, 192), (250, 240), (300, 288)];

pub const DEFAULT_DPI: u32 = 96;

/// The labels for the scale picker, in [`SCALES`] order.
pub fn labels() -> Vec<String> {
    SCALES.iter().map(|(pct, dpi)| format!("{pct}% ({dpi} DPI)")).collect()
}

/// Index in [`SCALES`] of `dpi`, or of the default when it is not one of them.
pub fn index_of(dpi: u32) -> usize {
    SCALES.iter().position(|&(_, d)| d == dpi).unwrap_or(0)
}

/// The registry values that set the DPI.
pub fn dpi_entries(dpi: u32) -> Vec<(String, String, RegData)> {
    [
        r"HKEY_CURRENT_USER\Control Panel\Desktop",
        r"HKEY_CURRENT_USER\Software\Wine\Fonts",
        r"HKEY_LOCAL_MACHINE\System\CurrentControlSet\Hardware Profiles\Current\Software\Fonts",
    ]
    .into_iter()
    .map(|key| (key.to_string(), "LogPixels".to_string(), RegData::Dword(dpi)))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_and_entries() {
        assert_eq!(SCALES[0], (100, DEFAULT_DPI));
        assert_eq!(labels()[2], "150% (144 DPI)");
        assert_eq!(index_of(144), 2);
        assert_eq!(index_of(97), 0, "an unknown value shows as the default");
        let e = dpi_entries(144);
        assert_eq!(e.len(), 3);
        assert!(e.iter().all(|(_, name, data)| name == "LogPixels" && *data == RegData::Dword(144)));
        assert!(e[0].0.ends_with(r"Control Panel\Desktop"));
    }
}
