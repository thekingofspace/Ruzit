use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread;

use base64::{Engine, engine::general_purpose::STANDARD as B64};

use crate::config::FileType;
use crate::package::{
    load_assets_only, load_single_managed_package, merge_pending_shard_into, LoadedPackage,
    ManagedSource, PackageAssetEntry, PendingShard,
};

pub struct Package {
    pub id: String,
    pub name: String,
    pub version: String,
    pub creator: String,
    pub entry: String,
    #[allow(dead_code)]
    pub file_type: FileType,

    pub physical_root: Option<PathBuf>,
    pub files: HashMap<String, String>,

    pub assets: RwLock<Option<HashMap<String, PackageAssetEntry>>>,
    pub scripts_compressed: bool,
    pub scripts_bytecode: bool,
    pub encryption_token: String,
    pub sources: Vec<ManagedSource>,
    pub activated: AtomicBool,
    pub is_default: bool,
    pub package_flags: Vec<String>,
    pub pending_shards: RwLock<Vec<PendingShard>>,
}

impl Package {
    pub fn from_loaded(lp: LoadedPackage) -> Self {
        Self {
            id: lp.id,
            name: lp.name,
            version: lp.version,
            creator: lp.creator,
            entry: lp.entry,
            file_type: lp.file_type,
            physical_root: lp.physical_root,
            files: lp.files,
            assets: RwLock::new(Some(lp.assets)),
            scripts_compressed: lp.scripts_compressed,
            scripts_bytecode: lp.scripts_bytecode,
            encryption_token: lp.encryption_token,
            sources: lp.sources,
            activated: AtomicBool::new(false),
            is_default: false,
            package_flags: lp.package_flags,
            pending_shards: RwLock::new(lp.pending_shards),
        }
    }

    pub fn package_flags(&self) -> &[String] {
        &self.package_flags
    }

    pub fn listens_for_flag(&self, flag: &str) -> bool {
        self.package_flags.iter().any(|f| f == flag)
    }

    pub fn pending_shard_flags(&self) -> Vec<Vec<String>> {
        self.pending_shards
            .read()
            .map(|g| g.iter().map(|s| s.flags.clone()).collect())
            .unwrap_or_default()
    }

    pub fn load_shards_for_flag(&self, flag: &str) -> Result<usize, String> {
        let sources_to_load: Vec<ManagedSource> = {
            let g = self.pending_shards.read().map_err(|e| format!("{e}"))?;
            g.iter()
                .filter(|s| s.flags.iter().any(|f| f == flag))
                .map(|s| s.source.clone())
                .collect()
        };
        if sources_to_load.is_empty() {
            return Ok(0);
        }
        let mut g = self.assets.write().map_err(|e| format!("{e}"))?;
        if g.is_none() {
            *g = Some(HashMap::new());
        }
        let map = g.as_mut().unwrap();
        let mut count = 0usize;
        for src in &sources_to_load {
            merge_pending_shard_into(map, src)?;
            count += 1;
        }
        Ok(count)
    }

    pub fn is_activated(&self) -> bool {
        self.is_default || self.activated.load(Ordering::Acquire)
    }

    pub fn activate(&self) {
        self.activated.store(true, Ordering::Release);
    }

    pub fn deactivate(&self) {
        self.activated.store(false, Ordering::Release);
    }

    pub fn assets_loaded(&self) -> bool {
        self.assets.read().map(|g| g.is_some()).unwrap_or(false)
    }

    pub fn asset_keys(&self) -> Vec<String> {
        let g = match self.assets.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        match g.as_ref() {
            Some(m) => {
                let mut v: Vec<String> = m.keys().cloned().collect();
                v.sort();
                v
            }
            None => Vec::new(),
        }
    }

    pub fn has_asset(&self, key: &str) -> bool {
        let g = match self.assets.read() {
            Ok(g) => g,
            Err(_) => return false,
        };
        g.as_ref().map(|m| m.contains_key(key)).unwrap_or(false)
    }

    pub fn read_asset(&self, key: &str) -> Result<Vec<u8>, String> {
        let g = self.assets.read().map_err(|e| format!("asset lock: {e}"))?;
        let map = g
            .as_ref()
            .ok_or_else(|| format!("package '{}' assets are not loaded (call manifest.LoadManaged first)", self.id))?;
        let entry = map
            .get(key)
            .ok_or_else(|| format!("asset '{}' not in package '{}'", key, self.id))?;
        let raw = B64
            .decode(entry.data_b64.as_bytes())
            .map_err(|e| format!("asset '{key}' base64: {e}"))?;
        let decrypted = if entry.encryption.eq_ignore_ascii_case("xor") {
            crate::managed::xor_with_token(&raw, &self.encryption_token)
        } else {
            raw
        };
        if entry.compressed {
            zstd::stream::decode_all(decrypted.as_slice())
                .map_err(|e| format!("asset '{key}' zstd: {e}"))
        } else {
            Ok(decrypted)
        }
    }

    pub fn free_assets(&self) {
        if let Ok(mut g) = self.assets.write() {
            *g = None;
        }
    }

    pub fn reload_assets(&self) -> Result<(), String> {
        if self.sources.is_empty() {
            return Ok(());
        }
        let (map, pending, _flags) = load_assets_only(&self.sources)?;
        if let Ok(mut g) = self.assets.write() {
            *g = Some(map);
        }
        if let Ok(mut p) = self.pending_shards.write() {
            *p = pending;
        }
        Ok(())
    }
}

enum EntryState {
    Pending { sources: Vec<ManagedSource> },
    Loading,
    Ready(Arc<Package>),
    #[allow(dead_code)]
    Failed(String),
}

struct PackageEntry {
    id: String,
    state: Mutex<EntryState>,
    cv: Condvar,
}

pub struct LazyPackageRegistry {
    entries: HashMap<String, Arc<PackageEntry>>,
    queue: Arc<(Mutex<VecDeque<String>>, Condvar)>,
    force_active: bool,
    package_flag_index: HashMap<String, Vec<String>>,
}

impl LazyPackageRegistry {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            queue: Arc::new((Mutex::new(VecDeque::new()), Condvar::new())),
            force_active: false,
            package_flag_index: HashMap::new(),
        }
    }

    pub fn new_test_mode() -> Self {
        Self {
            entries: HashMap::new(),
            queue: Arc::new((Mutex::new(VecDeque::new()), Condvar::new())),
            force_active: true,
            package_flag_index: HashMap::new(),
        }
    }

    pub fn set_package_flags(&mut self, id: String, flags: Vec<String>) {
        if !flags.is_empty() {
            self.package_flag_index.insert(id, flags);
        }
    }

    pub fn is_test_mode(&self) -> bool {
        self.force_active
    }

    pub fn insert_ready(&mut self, id: String, pkg: Arc<Package>) {
        let entry = Arc::new(PackageEntry {
            id: id.clone(),
            state: Mutex::new(EntryState::Ready(pkg)),
            cv: Condvar::new(),
        });
        self.entries.insert(id, entry);
    }

    pub fn insert_pending(&mut self, id: String, sources: Vec<ManagedSource>) {
        let entry = Arc::new(PackageEntry {
            id: id.clone(),
            state: Mutex::new(EntryState::Pending { sources }),
            cv: Condvar::new(),
        });
        self.entries.insert(id.clone(), entry);
        let (q, qcv) = &*self.queue;
        q.lock().unwrap().push_back(id);
        qcv.notify_one();
    }

    pub fn get(&self, id: &str) -> Option<Arc<Package>> {
        let entry = self.entries.get(id)?.clone();
        let queue = self.queue.clone();
        let mut state = entry.state.lock().unwrap();
        loop {
            match &*state {
                EntryState::Ready(pkg) => return Some(pkg.clone()),
                EntryState::Failed(_) => return None,
                EntryState::Pending { .. } | EntryState::Loading => {
                    let (q, qcv) = &*queue;
                    {
                        let mut q = q.lock().unwrap();
                        q.retain(|i| i != id);
                        q.push_front(id.to_string());
                        qcv.notify_one();
                    }
                    state = entry.cv.wait(state).unwrap();
                }
            }
        }
    }

    pub fn contains_key(&self, id: &str) -> bool {
        self.entries.contains_key(id)
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn pending_count(&self) -> usize {
        self.entries
            .values()
            .filter(|e| {
                matches!(
                    *e.state.lock().unwrap(),
                    EntryState::Pending { .. } | EntryState::Loading
                )
            })
            .count()
    }

    pub fn get_activated(&self, id: &str) -> Option<Arc<Package>> {
        let pkg = self.get(id)?;
        if self.force_active || pkg.is_activated() {
            Some(pkg)
        } else {
            None
        }
    }

    pub fn activate(&self, id: &str) -> Result<Arc<Package>, String> {
        let pkg = self
            .get(id)
            .ok_or_else(|| format!("manifest: package '{id}' is not registered"))?;
        if !pkg.assets_loaded() {
            pkg.reload_assets()?;
        }
        pkg.activate();
        Ok(pkg)
    }

    pub fn free_assets(&self, id: &str) -> Option<Arc<Package>> {
        let pkg = self.get(id)?;
        pkg.free_assets();
        Some(pkg)
    }

    pub fn call_flag(&self, flag: &str) -> CallFlagSummary {
        let mut summary = CallFlagSummary::default();
        let ids: Vec<String> = self.package_flag_index.keys().cloned().collect();
        for id in ids {
            let flags = match self.package_flag_index.get(&id) {
                Some(f) => f.clone(),
                None => continue,
            };
            if flags.iter().any(|f| f == flag) {
                let already_activated = self
                    .entries
                    .get(&id)
                    .and_then(|e| {
                        let s = e.state.lock().unwrap();
                        if let EntryState::Ready(p) = &*s {
                            Some(p.is_activated())
                        } else {
                            None
                        }
                    })
                    .unwrap_or(false);
                if !already_activated {
                    match self.activate(&id) {
                        Ok(_) => summary.packages_activated.push(id.clone()),
                        Err(e) => summary.errors.push(format!("{id}: {e}")),
                    }
                }
            }
        }

        for (id, entry) in &self.entries {
            let pkg = {
                let s = entry.state.lock().unwrap();
                match &*s {
                    EntryState::Ready(p) => p.clone(),
                    _ => continue,
                }
            };
            if !pkg.is_activated() && !self.force_active {
                continue;
            }
            match pkg.load_shards_for_flag(flag) {
                Ok(0) => {}
                Ok(n) => summary.shards_loaded.push((id.clone(), n)),
                Err(e) => summary.errors.push(format!("{id}: {e}")),
            }
        }
        summary
    }
}

#[derive(Default, Debug)]
pub struct CallFlagSummary {
    pub packages_activated: Vec<String>,
    pub shards_loaded: Vec<(String, usize)>,
    pub errors: Vec<String>,
}

pub fn spawn_lazy_loaders(registry: Arc<LazyPackageRegistry>, worker_count: usize) {
    let workers = worker_count.max(1);
    for _ in 0..workers {
        let reg = registry.clone();
        thread::spawn(move || loader_worker(reg));
    }
}

fn loader_worker(registry: Arc<LazyPackageRegistry>) {
    loop {
        let id = {
            let (q, qcv) = &*registry.queue;
            let mut qg = q.lock().unwrap();
            loop {
                if let Some(id) = qg.pop_front() {
                    break id;
                }
                let all_done = registry.entries.values().all(|e| {
                    let s = e.state.lock().unwrap();
                    !matches!(*s, EntryState::Pending { .. } | EntryState::Loading)
                });
                if all_done {
                    return;
                }
                qg = qcv.wait(qg).unwrap();
            }
        };
        let entry = match registry.entries.get(&id) {
            Some(e) => e.clone(),
            None => continue,
        };
        let sources = {
            let mut s = entry.state.lock().unwrap();
            match &mut *s {
                EntryState::Pending { sources } => {
                    let sources = std::mem::take(sources);
                    *s = EntryState::Loading;
                    sources
                }
                _ => continue,
            }
        };
        let result = load_single_managed_package(&entry.id, &sources);
        {
            let mut s = entry.state.lock().unwrap();
            *s = match result {
                Ok(loaded) => EntryState::Ready(Arc::new(Package::from_loaded(loaded))),
                Err(e) => EntryState::Failed(e),
            };
        }
        entry.cv.notify_all();
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub enum Fs {
    Disk {
        root: PathBuf,
        file_type: FileType,
        packages: Option<Arc<LazyPackageRegistry>>,
        default_id: Option<String>,
    },
    Bundle {
        packages: Arc<LazyPackageRegistry>,
        default_id: String,
        file_type: FileType,

        physical_root: PathBuf,
    },
}

impl Fs {
    pub fn file_type(&self) -> FileType {
        match self {
            Fs::Disk { file_type, .. } | Fs::Bundle { file_type, .. } => *file_type,
        }
    }
}

pub fn split_owner<'a>(owner: &'a str, default_id: &'a str) -> (&'a str, &'a str) {
    if let Some(rest) = owner.strip_prefix('@') {
        if let Some((id, inner)) = rest.split_once('/') {
            return (id, inner);
        }
    }
    (default_id, owner)
}

pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            Component::Normal(s) => out.push(s),
            Component::RootDir => out.push("/"),
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
        }
    }
    out
}

pub fn strip_anchors(name: &str) -> &str {
    let mut s = name;
    loop {
        if let Some(rest) = s.strip_prefix("./") {
            s = rest;
        } else if let Some(rest) = s.strip_prefix("../") {
            s = rest;
        } else if let Some(rest) = s.strip_prefix('/') {
            s = rest;
        } else {
            break;
        }
    }
    s
}

pub fn resolve(fs: &Fs, caller: &str, name: &str) -> Option<String> {
    match fs {
        Fs::Disk {
            root,
            file_type,
            packages,
            default_id,
        } => disk_resolve(
            root,
            caller,
            name,
            *file_type,
            packages.as_ref(),
            default_id.as_deref(),
        ),
        Fs::Bundle {
            packages,
            default_id,
            file_type,
            ..
        } => bundle_resolve(packages, default_id, *file_type, caller, name),
    }
}

pub fn init_anchor(inner_path: &str) -> String {
    let p = Path::new(inner_path);
    let basename = p
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if basename == "init.luau" || basename == "init.lua" {
        p.parent()
            .map(|d| d.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default()
    } else {
        String::new()
    }
}

pub fn read_module(fs: &Fs, key: &str) -> Option<Vec<u8>> {
    match fs {
        Fs::Disk {
            packages,
            default_id,
            ..
        } => {
            if key.starts_with('@') {
                let pkgs = packages.as_ref()?;
                let did = default_id.as_deref().unwrap_or("");
                return read_from_packages(pkgs, did, key);
            }
            fs::read(key).ok().map(strip_bom_bytes)
        }
        Fs::Bundle {
            packages,
            default_id,
            ..
        } => read_from_packages(packages, default_id, key),
    }
}

fn read_from_packages(
    packages: &LazyPackageRegistry,
    default_id: &str,
    key: &str,
) -> Option<Vec<u8>> {
    let (pkg_id, inner) = split_owner(key, default_id);
    let pkg = packages.get_activated(pkg_id)?;
    let stored = pkg.files.get(inner)?;
    if pkg.scripts_compressed {
        let compressed = B64.decode(stored.as_bytes()).ok()?;
        let bytes = zstd::stream::decode_all(compressed.as_slice()).ok()?;
        Some(if pkg.scripts_bytecode {
            bytes
        } else {
            strip_bom_bytes(bytes)
        })
    } else if pkg.scripts_bytecode {
        let bytes = B64.decode(stored.as_bytes()).ok()?;
        Some(bytes)
    } else {
        Some(strip_bom_bytes(stored.as_bytes().to_vec()))
    }
}

fn strip_bom_bytes(mut bytes: Vec<u8>) -> Vec<u8> {
    const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
    if bytes.starts_with(&BOM) {
        bytes.drain(..3);
    }
    bytes
}

#[allow(dead_code)]
fn strip_bom(s: String) -> String {
    const BOM: &str = "\u{feff}";
    if s.starts_with(BOM) {
        s[BOM.len()..].to_string()
    } else {
        s
    }
}

pub fn caller_dir(fs: &Fs, owner: &str) -> PathBuf {
    match fs {
        Fs::Disk { .. } => Path::new(owner)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default(),
        Fs::Bundle {
            packages,
            default_id,
            physical_root,
            ..
        } => {
            let (pkg_id, inner) = split_owner(owner, default_id);
            let pkg_root = packages
                .get(pkg_id)
                .and_then(|p| p.physical_root.clone())
                .unwrap_or_else(|| physical_root.clone());
            let v_parent = Path::new(inner).parent().unwrap_or(Path::new(""));
            pkg_root.join(v_parent)
        }
    }
}

pub fn fs_root(fs: &Fs) -> PathBuf {
    match fs {
        Fs::Disk { root, .. } => root.clone(),
        Fs::Bundle { physical_root, .. } => physical_root.clone(),
    }
}

pub fn physical_path(fs: &Fs, owner: &str, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    let resolved = match fs.file_type() {
        FileType::Relative => caller_dir(fs, owner).join(path),
        FileType::Global => fs_root(fs).join(strip_anchors(path)),
    };
    normalize(&resolved)
}

fn disk_resolve(
    root: &Path,
    caller: &str,
    name: &str,
    file_type: FileType,
    packages: Option<&Arc<LazyPackageRegistry>>,
    default_id: Option<&str>,
) -> Option<String> {
    if let Some(rest) = name.strip_prefix('@') {
        // `@Alias` (no slash) is shorthand for `@Alias/` -- resolve the
        // package's / alias's root, which `disk_lookup` and `bundle_lookup`
        // then turn into `init.luau` (matching Luau convention).
        let (alias, inner) = match rest.split_once('/') {
            Some((a, i)) => (a, i),
            None => (rest, ""),
        };
        let inner_clean = strip_anchors(inner);

        if let (Some(packages), Some(default_id)) = (packages, default_id) {
            let target_id = match alias {
                "self" => None,
                "Game" | "game" => Some(default_id.to_string()),
                other => Some(other.to_string()),
            };
            if let Some(target_id) = target_id {
                let pkg = packages.get_activated(&target_id)?;
                let key = bundle_lookup(&pkg.files, Path::new(inner_clean))?;
                return Some(if target_id == default_id {
                    format!("@{default_id}/{key}")
                } else {
                    format!("@{target_id}/{key}")
                });
            }
        }

        let base: PathBuf = match alias {
            "self" => {
                let caller_path = Path::new(caller);
                let basename = caller_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                if basename == "init.luau" || basename == "init.lua" {
                    caller_path
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| root.to_path_buf())
                        .join(inner_clean)
                } else {
                    root.join(inner_clean)
                }
            }
            "Game" | "game" => root.join(inner_clean),
            _ => return None,
        };
        return disk_lookup(&base);
    }
    match file_type {
        FileType::Relative => {
            let dir = Path::new(caller).parent()?;
            disk_lookup(&dir.join(name))
        }
        FileType::Global => disk_lookup(&root.join(strip_anchors(name))),
    }
}

fn disk_lookup(base: &Path) -> Option<String> {
    for ext in ["luau", "lua"] {
        let mut p = base.to_path_buf();
        p.set_extension(ext);
        if p.is_file() {
            return p.canonicalize().ok().map(path_to_string);
        }
    }
    for init in ["init.luau", "init.lua"] {
        let p = base.join(init);
        if p.is_file() {
            return p.canonicalize().ok().map(path_to_string);
        }
    }
    None
}

fn bundle_resolve(
    packages: &LazyPackageRegistry,
    default_id: &str,
    _file_type_unused: FileType,
    caller: &str,
    name: &str,
) -> Option<String> {
    if let Some(rest) = name.strip_prefix('@') {
        let (alias, inner) = match rest.split_once('/') {
            Some((a, i)) => (a, i),
            None => (rest, ""),
        };
        let (caller_pkg_id, caller_inner) = split_owner(caller, default_id);
        let target_id: &str = match alias {
            "self" => caller_pkg_id,
            "Game" | "game" => default_id,
            other => other,
        };
        let pkg = if alias == "self" {
            packages.get(target_id)?
        } else {
            packages.get_activated(target_id)?
        };
        let inner_clean = strip_anchors(inner);
        let base = if alias == "self" {
            let anchor = init_anchor(caller_inner);
            if anchor.is_empty() {
                Path::new(inner_clean).to_path_buf()
            } else {
                Path::new(&anchor).join(inner_clean)
            }
        } else {
            Path::new(inner_clean).to_path_buf()
        };
        let key = bundle_lookup(&pkg.files, &base)?;
        return Some(if target_id == default_id {
            key
        } else {
            format!("@{target_id}/{key}")
        });
    }

    let (caller_pkg_id, caller_inner) = split_owner(caller, default_id);
    let pkg = packages.get(caller_pkg_id)?;
    let base = match pkg.file_type {
        FileType::Relative => {
            let dir = Path::new(caller_inner).parent().unwrap_or(Path::new(""));
            dir.join(name)
        }
        FileType::Global => Path::new(strip_anchors(name)).to_path_buf(),
    };
    let key = bundle_lookup(&pkg.files, &base)?;
    if caller_pkg_id == default_id {
        Some(key)
    } else {
        Some(format!("@{caller_pkg_id}/{key}"))
    }
}

fn bundle_lookup(files: &HashMap<String, String>, base: &Path) -> Option<String> {
    let normalized = normalize(base);
    let base_str = normalized.to_string_lossy().replace('\\', "/");
    for ext in ["luau", "lua"] {
        let key = if base_str.is_empty() {
            format!(".{ext}")
        } else {
            format!("{base_str}.{ext}")
        };
        if files.contains_key(&key) {
            return Some(key);
        }
    }
    for init in ["init.luau", "init.lua"] {
        let key = if base_str.is_empty() {
            init.to_string()
        } else {
            format!("{base_str}/{init}")
        };
        if files.contains_key(&key) {
            return Some(key);
        }
    }
    None
}

fn path_to_string(p: PathBuf) -> String {
    p.to_string_lossy().into_owned()
}
