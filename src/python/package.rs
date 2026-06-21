use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageState {
    Installing,
    Installed,
    Waiting,
    NotInstalled,
    Err,
    Builtin,
}

impl PackageState {
    pub fn as_str(&self) -> &'static str {
        match self {
            PackageState::Installing => "installing",
            PackageState::Installed => "installed",
            PackageState::Waiting => "waiting",
            PackageState::NotInstalled => "not_installed",
            PackageState::Err => "err",
            PackageState::Builtin => "builtin",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PackageData {
    pub name: String,
    pub desc: Option<String>,
    pub url: Option<String>,
    pub version: Option<String>,
    pub detail: Option<String>,
    pub pip_source: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Package {
    pub name: String,
    pub desc: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pip_source: Option<String>,
}

impl Package {
    pub fn new(name: String, desc: String, state: PackageState) -> Self {
        Self {
            name,
            desc,
            state: state.as_str().to_string(),
            url: None,
            version: None,
            detail: None,
            pip_source: None,
        }
    }

    pub fn with_version(
        name: String,
        desc: String,
        state: PackageState,
        version: Option<String>,
    ) -> Self {
        let mut p = Self::new(name, desc, state);
        p.version = version;
        p
    }

    pub fn change_state(&mut self, new_state: PackageState) {
        self.state = new_state.as_str().to_string();
    }

    pub fn from_dict(d: PackageData) -> Self {
        Self {
            name: d.name,
            desc: d.desc.unwrap_or_default(),
            state: PackageState::NotInstalled.as_str().to_string(),
            url: d.url,
            version: d.version,
            detail: d.detail,
            pip_source: d.pip_source,
        }
    }
}
