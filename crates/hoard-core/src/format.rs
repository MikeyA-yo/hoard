//! Human-readable formatting shared by the CLI and the desktop app.

const SHORT_UNITS: [&str; 7] = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];
const IEC_UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

/// Formats a byte count with binary (1024) multiples and short labels,
/// e.g. `1.4 GB`. This is the default display style.
pub fn bytes(n: u64) -> String {
    format_with(n, &SHORT_UNITS)
}

/// Same as [`bytes`] but with IEC labels, e.g. `1.4 GiB`.
pub fn bytes_iec(n: u64) -> String {
    format_with(n, &IEC_UNITS)
}

fn format_with(n: u64, units: &[&str; 7]) -> String {
    if n < 1024 {
        return format!("{n} {}", units[0]);
    }
    let mut value = n as f64;
    let mut unit = 0;
    // Move up a unit if rounding to one decimal would print "1024.0".
    while (value >= 1024.0 || format!("{value:.1}") == "1024.0") && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", units[unit])
}

/// Formats an integer with thousands separators, e.g. `1,234,567`.
pub fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.0 KB");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(1024 * 1024 - 1), "1.0 MB");
        assert_eq!(bytes(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
        assert_eq!(bytes(u64::MAX), "16.0 EB");
        assert_eq!(bytes_iec(1024 * 1024), "1.0 MiB");
    }

    #[test]
    fn formats_counts() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1000), "1,000");
        assert_eq!(count(1_234_567), "1,234,567");
    }
}
