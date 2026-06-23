use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub html_url: String,
    pub prerelease: bool,
    pub draft: bool,
    pub published_at: Option<String>,
    pub assets: Vec<ReleaseAsset>,
}

pub fn target_asset_name() -> &'static str {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match (os, arch) {
        ("windows", "x86_64") => "xescoding-helper-windows-x86_64.exe",
        ("linux", "x86_64") => "xescoding-helper-linux-x86_64",
        ("macos", "x86_64") => "xescoding-helper-macos-x86_64",
        ("macos", "aarch64") => "xescoding-helper-macos-aarch64",
        _ => "",
    }
}

pub fn pick_asset<'a>(release: &'a ReleaseInfo) -> Option<&'a ReleaseAsset> {
    let target = target_asset_name();
    if target.is_empty() {
        return None;
    }
    release
        .assets
        .iter()
        .find(|a| a.name == target)
        .or_else(|| {
            let prefix = if cfg!(windows) {
                "xescoding-helper-windows"
            } else if cfg!(target_os = "macos") {
                "xescoding-helper-macos"
            } else {
                "xescoding-helper-linux"
            };
            release.assets.iter().find(|a| a.name.starts_with(prefix))
        })
}

pub async fn fetch_latest_release(
    client: &reqwest::Client,
    repo: &str,
    include_prerelease: bool,
) -> Result<ReleaseInfo, String> {
    let url = format!(
        "https://api.github.com/repos/{repo}/releases?per_page=20"
    );
    let resp = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "xescoding-helper-updater")
        .send()
        .await
        .map_err(|e| format!("请求 GitHub 失败: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取响应失败: {e}"))?;
    if !status.is_success() {
        return Err(format!("GitHub API 错误 ({}): {}", status.as_u16(), text));
    }
    let releases: Vec<ReleaseInfo> =
        serde_json::from_str(&text).map_err(|e| format!("解析失败: {e}"))?;
    releases
        .into_iter()
        .find(|r| !r.draft && (include_prerelease || !r.prerelease))
        .ok_or_else(|| "未找到合适的发布".to_string())
}

pub fn parse_version(tag: &str) -> Option<semver::Version> {
    let s = tag.trim_start_matches('v');
    semver::Version::parse(s).ok()
}
