//! Own test binary so RSS isn't skewed by the large decodes in the unit tests.

/// Failed opens must release the LibRaw handle (~700 KB each).
#[test]
fn failed_open_does_not_leak() {
    fn rss_kb() -> Option<u64> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
        line.split_whitespace().nth(1)?.parse().ok()
    }
    let garbage = b"definitely not a raw file";
    for _ in 0..50 {
        assert!(rsraw::RawImage::open(garbage).is_err());
    }
    let before = rss_kb();
    for _ in 0..1000 {
        assert!(rsraw::RawImage::open(garbage).is_err());
    }
    if let (Some(before), Some(after)) = (before, rss_kb()) {
        // A leaked handle per open would grow RSS by roughly 700 MB here.
        assert!(
            after.saturating_sub(before) < 50_000,
            "{before} KB -> {after} KB"
        );
    }
}
