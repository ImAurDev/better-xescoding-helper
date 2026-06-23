use std::time::Duration;

pub fn sample_rss_bytes(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        sample_rss_linux(pid)
    }
    #[cfg(target_os = "windows")]
    {
        sample_rss_windows(pid)
    }
    #[cfg(target_os = "macos")]
    {
        sample_rss_macos(pid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

#[cfg(target_os = "linux")]
fn sample_rss_linux(pid: u32) -> Option<u64> {
    let path = format!("/proc/{}/status", pid);
    let content = std::fs::read_to_string(&path).ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if let Some(kb_str) = parts.first() {
                if let Ok(kb) = kb_str.parse::<u64>() {
                    return Some(kb * 1024);
                }
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn sample_rss_macos(pid: u32) -> Option<u64> {
    use std::process::Command;
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout);
    let kb: u64 = s.trim().parse().ok()?;
    Some(kb * 1024)
}

#[cfg(target_os = "windows")]
fn sample_rss_windows(pid: u32) -> Option<u64> {
    use std::process::Command;
    let output = Command::new("tasklist")
        .args([
            "/FI",
            &format!("PID eq {}", pid),
            "/FO",
            "CSV",
            "/NH",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?;
    let mut in_quotes = false;
    let mut fields: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in line.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    fields.push(cur);
    if fields.len() < 5 {
        return None;
    }
    let mem_str = fields[4].replace(',', "").replace(" K", "").replace("\"", "");
    let kb: u64 = mem_str.trim().parse().ok()?;
    Some(kb * 1024)
}

pub fn sample_process_tree_rss(pid: u32) -> Option<u64> {
    sample_rss_bytes(pid)
}

pub async fn poll_peak_rss<F, FutFn, Fut>(
    pid_getter: F,
    mut should_continue: FutFn,
    interval: Duration,
) -> u64
where
    F: Fn() -> Option<u32>,
    FutFn: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let mut peak = 0u64;
    loop {
        if !should_continue().await {
            break;
        }
        if let Some(pid) = pid_getter() {
            if let Some(rss) = sample_rss_bytes(pid) {
                if rss > peak {
                    peak = rss;
                }
            }
        }
        tokio::time::sleep(interval).await;
    }
    peak
}
