//! Windows implementation: CPU sets, Job Object pinning, cache purging,
//! memory ballast.

use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_NO_BUFFERING, FlushFileBuffers};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_AFFINITY,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows_sys::Win32::System::SystemInformation::{
    GetSystemCpuSetInformation, GlobalMemoryStatusEx, MEMORYSTATUSEX, SYSTEM_CPU_SET_INFORMATION,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

struct CpuSet {
    logical: u8,
    core: u8,
    efficiency_class: u8,
}

fn cpu_sets() -> Result<Vec<CpuSet>, String> {
    let mut len = 0u32;
    // SAFETY: a null buffer with length 0 asks for the required size.
    unsafe {
        GetSystemCpuSetInformation(std::ptr::null_mut(), 0, &mut len, GetCurrentProcess(), 0)
    };
    let mut buf = vec![0u8; len as usize];
    // SAFETY: buf has the size the previous call asked for.
    let ok = unsafe {
        GetSystemCpuSetInformation(
            buf.as_mut_ptr().cast(),
            len,
            &mut len,
            GetCurrentProcess(),
            0,
        )
    };
    if ok == 0 {
        return Err("GetSystemCpuSetInformation failed".into());
    }
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < len as usize {
        // SAFETY: entries are laid out back to back, each `Size` bytes long,
        // within the `len` bytes the call filled in.
        let entry = unsafe { &*buf.as_ptr().add(at).cast::<SYSTEM_CPU_SET_INFORMATION>() };
        // SAFETY: Type 0 (CpuSetInformation) entries use the CpuSet variant.
        let cs = unsafe { entry.Anonymous.CpuSet };
        out.push(CpuSet {
            logical: cs.LogicalProcessorIndex,
            core: cs.CoreIndex,
            efficiency_class: cs.EfficiencyClass,
        });
        at += entry.Size as usize;
    }
    Ok(out)
}

/// Affinity mask of the highest-performance cores (largest efficiency class).
fn performance_mask() -> Result<(usize, Vec<u8>), String> {
    let sets = cpu_sets()?;
    let best = sets.iter().map(|c| c.efficiency_class).max().unwrap_or(0);
    let logical: Vec<u8> = sets
        .iter()
        .filter(|c| c.efficiency_class == best)
        .map(|c| c.logical)
        .collect();
    Ok((
        logical.iter().fold(0usize, |m, &l| m | (1usize << l)),
        logical,
    ))
}

fn power() -> serde_json::Value {
    // SAFETY: SYSTEM_POWER_STATUS is plain data; all-zero is a valid value.
    let mut s: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: s is a valid out-parameter.
    let ok = unsafe { GetSystemPowerStatus(&mut s) };
    serde_json::json!({
        "ok": ok != 0,
        "onAc": s.ACLineStatus == 1,
        "batteryPercent": s.BatteryLifePercent,
    })
}

pub fn info() -> Result<serde_json::Value, String> {
    let sets = cpu_sets()?;
    let (mask, pcores) = performance_mask()?;
    // SAFETY: MEMORYSTATUSEX is plain data; all-zero is a valid value.
    let mut mem: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    mem.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: dwLength is set as the API requires.
    unsafe { GlobalMemoryStatusEx(&mut mem) };
    Ok(serde_json::json!({
        "cpuSets": sets.iter().map(|c| serde_json::json!({
            "logical": c.logical, "core": c.core, "efficiencyClass": c.efficiency_class
        })).collect::<Vec<_>>(),
        "performanceCores": pcores,
        "performanceMask": format!("{mask:#x}"),
        "memory": {
            "totalBytes": mem.ullTotalPhys,
            "availableBytes": mem.ullAvailPhys,
        },
        "power": power(),
    }))
}

/// Runs `program` inside a Job Object. With `pcores`, every process in the
/// job (the program and everything it starts, e.g. WebView2) is limited to
/// the performance cores. `logical` is passed on as `QUIRE_CPU_LOGICAL` so
/// the app sizes its pools for the emulated machine.
pub fn run(program: &[String], pcores: bool, logical: Option<u32>) -> Result<i32, String> {
    // SAFETY: plain Win32 calls with valid arguments; the job handle stays
    // open (and the job alive) until this process exits.
    unsafe {
        let job: HANDLE = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err("CreateJobObjectW failed".into());
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if pcores {
            let (mask, _) = performance_mask()?;
            limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_AFFINITY;
            limits.BasicLimitInformation.Affinity = mask;
        }
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            return Err("SetInformationJobObject failed".into());
        }
        // Join the job ourselves; children inherit membership, so the
        // program and every process it starts are covered.
        if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
            return Err("AssignProcessToJobObject failed".into());
        }
    }
    let mut cmd = std::process::Command::new(&program[0]);
    cmd.args(&program[1..]);
    if let Some(n) = logical {
        cmd.env("QUIRE_CPU_LOGICAL", n.to_string());
    }
    let status = cmd
        .status()
        .map_err(|e| format!("failed to start {}: {e}", program[0]))?;
    Ok(status.code().unwrap_or(1))
}

/// Opening a file for unbuffered I/O makes the cache manager flush and purge
/// (directories are walked recursively)
/// its cached pages, as long as no other process has the file mapped.
/// Measured, not assumed: `tools/bench/refsim.ps1` checks the cold read
/// speed afterwards.
pub fn purge(paths: &[String]) -> Result<serde_json::Value, String> {
    // Directories are purged recursively.
    let mut files = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = paths.iter().map(Into::into).collect();
    while let Some(p) = stack.pop() {
        match std::fs::read_dir(&p) {
            Ok(entries) => stack.extend(entries.flatten().map(|e| e.path())),
            Err(_) => files.push(p.to_string_lossy().into_owned()),
        }
    }
    let mut results = Vec::new();
    for f in &files {
        let r = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_NO_BUFFERING)
            .open(f)
            .map(|file| {
                // SAFETY: the handle is valid for the file's lifetime.
                unsafe { FlushFileBuffers(file.as_raw_handle() as HANDLE) };
                drop(file);
            });
        results.push((f, r));
    }
    let failed: Vec<serde_json::Value> = results
        .iter()
        .filter_map(|(f, r)| {
            r.as_ref()
                .err()
                .map(|e| serde_json::json!({ "file": f, "error": e.to_string() }))
        })
        .collect();
    Ok(serde_json::json!({
        "files": results.len(),
        "purged": results.len() - failed.len(),
        "failed": failed.len(),
        "failures": failed.into_iter().take(20).collect::<Vec<_>>(),
    }))
}

/// Commits `gib` GiB and keeps it resident by touching every page once a
/// second, so the system behaves as if it had that much less RAM.
pub fn ballast(gib: f64, seconds: u64) -> Result<serde_json::Value, String> {
    let bytes = (gib * (1u64 << 30) as f64) as usize;
    // SAFETY: allocating fresh committed memory; only written through the
    // returned pointer within `bytes`.
    let ptr = unsafe {
        VirtualAlloc(
            std::ptr::null(),
            bytes,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        )
    };
    if ptr.is_null() {
        return Err(format!("could not commit {gib} GiB"));
    }
    let base = ptr.cast::<u8>();
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut round = 0u8;
    while Instant::now() < deadline {
        round = round.wrapping_add(1);
        for page in (0..bytes).step_by(4096) {
            // SAFETY: page < bytes, inside the committed region.
            unsafe { base.add(page).write_volatile(round) };
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    // The memory is released when the process exits.
    Ok(serde_json::json!({ "heldGiB": gib, "seconds": seconds }))
}
