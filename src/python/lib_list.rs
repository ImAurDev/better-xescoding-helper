use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::python::package::{Package, PackageState};
use crate::python::package_manager::{InstallRequest, PackageManager};

const MUST_TYPE: &str = "must";
const OPTION_TYPE: &str = "option";
const CACHE_TYPE: &str = "cache";

#[derive(Clone)]
struct PackEntry {
    pack_type: String,
    index: usize,
}

pub struct PackageList {
    pkg_manager: PackageManager,
    state: PackageState,
    queue: Vec<String>,
    current_installing: Option<String>,
    packages: HashMap<String, Vec<Package>>,
    err_dict: HashMap<String, PackEntry>,
    name_dict: HashMap<String, PackEntry>,
    static_list: HashMap<String, i32>,
    desc_list: HashMap<String, String>,
    has_new_err: i32,
    install_tags: HashSet<String>,
    pre_states: HashMap<String, PackageState>,
    lock_id: Option<String>,
}

impl PackageList {
    pub fn new(pkg_manager: PackageManager) -> Self {
        let mut packages = HashMap::new();
        packages.insert(
            MUST_TYPE.to_string(),
            vec![
                Package::new(
                    "xes-lib".into(),
                    "xes-lib是学而思专用的python库，实现了很多实用的功能，包括发送、路径查询、预处理、文件操作等功能...".into(),
                    PackageState::NotInstalled,
                ),
                Package::new("qrcode".into(), "二维码编码解码库。".into(), PackageState::NotInstalled),
            ],
        );
        packages.insert(
            OPTION_TYPE.to_string(),
            vec![
                Package::new(
                    "Pillow".into(),
                    "一个图像处理库，可以对图像进行旋转、缩放、裁剪、增色等处理。".into(),
                    PackageState::NotInstalled,
                ),
                Package::new(
                    "numpy".into(),
                    "Python最常用的科学计算库，为Python提供了很多高级数学函数。".into(),
                    PackageState::NotInstalled,
                ),
                Package::new(
                    "algorithms".into(),
                    "一个 Python 算法库，提供了常用的数据结构和算法。".into(),
                    PackageState::NotInstalled,
                ),
            ],
        );
        packages.insert(CACHE_TYPE.to_string(), Vec::new());

        let mut name_dict = HashMap::new();
        for (key, list) in &packages {
            for (i, item) in list.iter().enumerate() {
                name_dict.insert(
                    item.name.clone(),
                    PackEntry {
                        pack_type: key.clone(),
                        index: i,
                    },
                );
            }
        }

        Self {
            pkg_manager,
            state: PackageState::Installed,
            queue: Vec::new(),
            current_installing: None,
            packages,
            err_dict: HashMap::new(),
            name_dict,
            static_list: HashMap::new(),
            desc_list: HashMap::new(),
            has_new_err: 0,
            install_tags: HashSet::new(),
            pre_states: HashMap::new(),
            lock_id: None,
        }
    }

    #[allow(dead_code)]
    pub fn pkg_manager(&self) -> PackageManager {
        self.pkg_manager.clone()
    }

    fn get_list(&self, ty: &str) -> &[Package] {
        self.packages.get(ty).map(|v| v.as_slice()).unwrap_or(&[])
    }

    fn get_list_mut(&mut self, ty: &str) -> &mut Vec<Package> {
        self.packages.entry(ty.to_string()).or_default()
    }

    fn change_state_by_name(&mut self, name: &str, state: PackageState) {
        if state == PackageState::Installing {
            self.current_installing = Some(name.to_string());
        }
        if let Some(entry) = self.name_dict.get(name).cloned() {
            if let Some(pack) = self.get_list_mut(&entry.pack_type).get_mut(entry.index) {
                pack.change_state(state);
            }
        }
    }

    fn get_package_by_name(&self, name: &str) -> Package {
        let entry = match self.name_dict.get(name) {
            Some(e) => e.clone(),
            None => {
                return Package::new(name.to_string(), String::new(), PackageState::NotInstalled)
            }
        };
        self.get_list(&entry.pack_type)
            .get(entry.index)
            .cloned()
            .unwrap_or_else(|| {
                Package::new(name.to_string(), String::new(), PackageState::NotInstalled)
            })
    }

    fn check_lock(&mut self, page_id: Option<&str>) -> bool {
        if self.state == PackageState::Installing {
            if self.lock_id.is_none() {
                self.lock_id = page_id.map(|s| s.to_string());
                return true;
            }
            if self.lock_id.as_deref() == page_id {
                return true;
            }
            if page_id.is_none() {
                return true;
            }
            return false;
        }
        self.lock_id = None;
        true
    }

    pub fn unlock(&mut self, page_id: &str) {
        if self.lock_id.as_deref() == Some(page_id) {
            self.lock_id = None;
        }
    }

    pub async fn get_pack_list(&mut self, page_id: &str) -> Value {
        if !self.check_lock(Some(page_id)) {
            return Value::Bool(false);
        }

        let (user_list, lib_list) = self.pkg_manager.get_local_list().await;
        let need_filter: HashSet<String> = lib_list.iter().map(|p| p.name.clone()).collect();
        let filtered: Vec<_> = user_list
            .iter()
            .filter(|p| !need_filter.contains(&p.name))
            .cloned()
            .collect();

        let mut last_option_list: Vec<Package> = Vec::new();
        let mut need_show: Vec<String> = Vec::new();

        for pack in &filtered {
            if self.err_dict.contains_key(&pack.name) {
                self.remove_err_pack(&pack.name);
            }
            let entry = self.name_dict.get(&pack.name).cloned();
            let desc = self.desc_list.get(&pack.name).cloned();
            if let Some(e) = &entry {
                if e.pack_type == MUST_TYPE || e.pack_type == OPTION_TYPE {
                    self.change_state_by_name(&pack.name, PackageState::Installed);
                    continue;
                }
            }
            let mut version = pack.version.clone();
            if let Some(d) = &desc {
                version = d.clone();
            } else {
                need_show.push(pack.name.clone());
            }
            last_option_list.push(Package::with_version(
                pack.name.clone(),
                desc.clone().unwrap_or_default(),
                PackageState::Installed,
                Some(version.clone()),
            ));
            if entry.is_none() {
                let cache_len = self.get_list(CACHE_TYPE).len();
                self.name_dict.insert(
                    pack.name.clone(),
                    PackEntry {
                        pack_type: CACHE_TYPE.to_string(),
                        index: cache_len,
                    },
                );
                self.get_list_mut(CACHE_TYPE).push(Package::with_version(
                    pack.name.clone(),
                    desc.clone().unwrap_or_default(),
                    PackageState::Installed,
                    Some(version),
                ));
            }
        }

        let must_arr: Vec<Package> = self.get_list(MUST_TYPE).to_vec();
        let mut option_arr: Vec<Package> = self.get_list(OPTION_TYPE).to_vec();
        option_arr.extend(last_option_list);

        let mut res = serde_json::Map::new();
        res.insert("state".into(), json!(self.state.as_str()));
        res.insert(MUST_TYPE.into(), json!(must_arr));
        res.insert(OPTION_TYPE.into(), json!(option_arr));

        let mut target = serde_json::Map::new();
        for (k, v) in &res {
            target.insert(k.clone(), v.clone());
        }
        if let Some(Value::Array(must)) = target.get_mut(MUST_TYPE) {
            must.retain(|p| p.get("name").and_then(|n| n.as_str()) != Some("xesrepair"));
        }
        let _ = need_show;
        Value::Object(target)
    }

    pub fn get_err_list(&self) -> Vec<Package> {
        let mut res = Vec::new();
        for (name, cache) in &self.err_dict {
            if let Some(pack) = self.get_list(&cache.pack_type).get(cache.index) {
                res.push(pack.clone());
            }
            let _ = name;
        }
        res
    }

    pub async fn get_all_state(&mut self, page_id: &str) -> Value {
        if !self.check_lock(Some(page_id)) {
            return Value::Bool(false);
        }
        json!({
            "all_state": self.state.as_str(),
            "err_count": self.err_dict.len(),
            "installing_count": (if self.current_installing.is_some() { 1 } else { 0 }) + self.queue.len(),
        })
    }

    pub async fn get_state(&mut self, _pre: &str, is_req: bool) -> Value {
        if is_req {
            return json!({});
        }
        let res = self.pkg_manager.get_process().await;
        let err_count = self.err_dict.len();
        let installing_count = (if self.current_installing.is_some() {
            1
        } else {
            0
        }) + self.queue.len();

        match res {
            None => {
                json!({
                    "all_state": self.state.as_str(),
                    "err_count": err_count,
                    "installing_count": installing_count,
                    "desc": "",
                    "has_new_err": false,
                    "tag": null,
                })
            }
            Some(info) => {
                if info.state == "installed" {
                    self.err_dict.remove(&info.name);
                    self.change_state_by_name(&info.name, PackageState::Installed);
                    self.check_next().await;
                } else if info.state == "error" {
                    self.has_new_err = 1;
                    if let Some(entry) = self.name_dict.get(&info.name).cloned() {
                        self.err_dict.insert(info.name.clone(), entry);
                    }
                    self.change_state_by_name(&info.name, PackageState::Err);
                    self.check_next().await;
                }
                let err_count = self.err_dict.len();
                let installing_count = (if self.current_installing.is_some() {
                    1
                } else {
                    0
                }) + self.queue.len();
                json!({
                    "name": info.name,
                    "progress": info.progress,
                    "state": info.state,
                    "msg": info.msg,
                    "all_state": self.state.as_str(),
                    "err_count": err_count,
                    "installing_count": installing_count,
                })
            }
        }
    }

    async fn check_next(&mut self) {
        if !self.queue.is_empty() {
            let next_p = self.queue.remove(0);
            self.current_installing = Some(next_p.clone());
            self.change_state_by_name(&next_p, PackageState::Installing);
            let pack = self.get_package_by_name(&next_p);
            let req = InstallRequest {
                name: pack.name.clone(),
                version: pack.version.clone(),
                url: pack.url.clone(),
                pip_source: pack.pip_source.clone(),
            };
            let pkg = self.pkg_manager.clone();
            tokio::spawn(async move {
                let _ = pkg.handle_install(req).await;
            });
        } else {
            self.current_installing = None;
            if !self.err_dict.is_empty() {
                if self.has_new_err == 1 {
                    self.has_new_err = 2;
                } else {
                    self.has_new_err = 0;
                }
                self.state = PackageState::Err;
            } else {
                self.state = PackageState::Installed;
            }
        }
    }

    pub async fn install_handler(
        &mut self,
        name: &str,
        version: &str,
        desc: &str,
        page_id: Option<&str>,
    ) -> Option<String> {
        self.install_tags.insert(name.to_string());
        if name == "xesrepair" {
            return if self.check_lock(page_id) {
                Some("true".to_string())
            } else {
                None
            };
        }

        self.state = PackageState::Installing;

        let entry = self.name_dict.get(name).cloned();
        if entry.is_none() {
            self.pre_states
                .insert(name.to_string(), PackageState::NotInstalled);
            let cache_len = self.get_list(CACHE_TYPE).len();
            self.get_list_mut(CACHE_TYPE).push(Package::new(
                name.to_string(),
                desc.to_string(),
                PackageState::NotInstalled,
            ));
            self.name_dict.insert(
                name.to_string(),
                PackEntry {
                    pack_type: CACHE_TYPE.to_string(),
                    index: cache_len,
                },
            );
        } else if let Some(e) = entry {
            let pre = self
                .get_list(&e.pack_type)
                .get(e.index)
                .map(|p| {
                    if p.state == PackageState::Err.as_str() {
                        PackageState::Err
                    } else {
                        PackageState::NotInstalled
                    }
                })
                .unwrap_or(PackageState::NotInstalled);
            self.pre_states.insert(name.to_string(), pre);
        }

        if !self.queue.is_empty() || self.current_installing.is_some() {
            self.queue.push(name.to_string());
            self.change_state_by_name(name, PackageState::Waiting);
            return Some(PackageState::Waiting.as_str().to_string());
        }

        self.change_state_by_name(name, PackageState::Installing);
        let pack = self.get_package_by_name(name);
        let req = InstallRequest {
            name: pack.name.clone(),
            version: if version.is_empty() {
                None
            } else {
                Some(version.to_string())
            }
            .or(pack.version.clone()),
            url: pack.url.clone(),
            pip_source: pack.pip_source.clone(),
        };
        let _ = self.pkg_manager.handle_install(req).await;
        Some(PackageState::Installing.as_str().to_string())
    }

    pub async fn cancel_install_handler(&mut self, name: &str) {
        let pre_state = self
            .pre_states
            .get(name)
            .copied()
            .unwrap_or(PackageState::NotInstalled);
        self.change_state_by_name(name, pre_state);

        if self.current_installing.as_deref() == Some(name) {
            self.pkg_manager.cancel_install();
            self.check_next().await;
        } else {
            if let Some(idx) = self.queue.iter().position(|n| n == name) {
                self.queue.remove(idx);
            }
        }
    }

    pub async fn uninstall_handler(&mut self, name: &str) {
        if self.name_dict.contains_key(name) {
            self.pkg_manager.handle_uninstall(name).await;
            self.change_state_by_name(name, PackageState::NotInstalled);
        }
    }

    pub fn remove_err_pack(&mut self, name: &str) {
        if let Some(cache) = self.err_dict.get(name).cloned() {
            if let Some(pack) = self.get_list_mut(&cache.pack_type).get_mut(cache.index) {
                pack.state = PackageState::NotInstalled.as_str().to_string();
            }
            self.err_dict.remove(name);
            if self.queue.is_empty() && self.err_dict.is_empty() {
                self.state = PackageState::Installed;
            }
        }
    }

    pub async fn search_handler(&mut self, name: &str, flag: bool) -> Value {
        let res_list: Vec<Package>;
        if flag && self.name_dict.contains_key(name) {
            res_list = vec![self.get_package_by_name(name)];
        } else {
            let search_result = self.pkg_manager.handle_search(name).await;
            res_list = search_result.into_iter().map(Package::from_dict).collect();
        }

        let mut must_arr: Vec<Package> = Vec::new();
        let mut option_arr: Vec<Package> = Vec::new();

        for pack in res_list.into_iter() {
            if self.static_list.contains_key(&pack.name) {
                let mut p = pack.clone();
                p.state = PackageState::Builtin.as_str().to_string();
                option_arr.push(p);
            } else if self.name_dict.contains_key(&pack.name) {
                let cache_pack = self.get_package_by_name(&pack.name);
                let entry = self.name_dict.get(&pack.name).cloned();
                if let Some(e) = &entry {
                    if e.pack_type == MUST_TYPE {
                        must_arr.push(cache_pack);
                    } else {
                        option_arr.push(cache_pack);
                    }
                } else {
                    option_arr.push(cache_pack);
                }
            } else {
                option_arr.push(pack.clone());
            }
            if flag {
                let cache_len = self.get_list(CACHE_TYPE).len();
                self.name_dict.insert(
                    pack.name.clone(),
                    PackEntry {
                        pack_type: CACHE_TYPE.to_string(),
                        index: cache_len,
                    },
                );
                self.get_list_mut(CACHE_TYPE).push(pack);
            }
        }

        let mut res = serde_json::Map::new();
        res.insert(MUST_TYPE.into(), json!(must_arr));
        res.insert(OPTION_TYPE.into(), json!(option_arr));
        Value::Object(res)
    }
}

pub async fn run_auto_state(pl: std::sync::Arc<Mutex<PackageList>>) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
    interval.tick().await;
    loop {
        interval.tick().await;
        let mut pl = pl.lock().await;
        if pl.current_installing.is_some() || !pl.queue.is_empty() {
            pl.get_state("", false).await;
        }
    }
}
