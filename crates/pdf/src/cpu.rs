//! How many PDF workers to run (§5.2, ADR-0036): one per physical
//! performance core, minus one for the UI and the host, at least one.
//!
//! On hybrid CPUs Windows reports an efficiency class per core; only cores
//! of the highest class count (the dev machine has 4 P-cores and 4
//! low-power E-cores: 3 workers). Elsewhere every physical core counts.
//! `QUIRE_PDF_WORKERS` overrides the result (ref-sim, tests).

/// Physical cores of the highest efficiency class, if the OS says.
pub fn performance_cores() -> Option<usize> {
    #[cfg(windows)]
    {
        windows_cores()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

pub fn worker_count() -> usize {
    if let Some(n) = std::env::var("QUIRE_PDF_WORKERS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
    {
        return n;
    }
    let cores = performance_cores().unwrap_or_else(|| {
        // Without core topology, assume two hardware threads per core.
        std::thread::available_parallelism().map_or(2, |n| n.get().div_ceil(2))
    });
    cores.saturating_sub(1).max(1)
}

#[cfg(windows)]
fn windows_cores() -> Option<usize> {
    use windows_sys::Win32::System::SystemInformation::{
        GetLogicalProcessorInformationEx, RelationProcessorCore,
        SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
    };
    let mut len = 0u32;
    // SAFETY: a size query with a null buffer, then a call with a buffer of
    // the size Windows asked for; records are read within that buffer.
    unsafe {
        GetLogicalProcessorInformationEx(RelationProcessorCore, std::ptr::null_mut(), &mut len);
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        let base = buf.as_mut_ptr().cast::<u8>();
        if GetLogicalProcessorInformationEx(RelationProcessorCore, base.cast(), &mut len) == 0 {
            return None;
        }
        let mut classes = Vec::new();
        let mut off = 0usize;
        while off + size_of::<u32>() * 2 <= len as usize {
            let rec = &*base
                .add(off)
                .cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>();
            if rec.Size == 0 {
                break;
            }
            if rec.Relationship == RelationProcessorCore {
                classes.push(rec.Anonymous.Processor.EfficiencyClass);
            }
            off += rec.Size as usize;
        }
        let top = *classes.iter().max()?;
        Some(classes.iter().filter(|&&c| c == top).count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_least_one_worker() {
        assert!(worker_count() >= 1);
        if let Some(n) = performance_cores() {
            assert!(n >= 1);
            let logical = std::thread::available_parallelism().unwrap().get();
            assert!(n <= logical, "{n} performance cores of {logical} threads");
        }
    }
}
