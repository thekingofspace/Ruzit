use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Relative,
    Global,
}

impl Default for FileType {
    fn default() -> Self {
        FileType::Relative
    }
}

impl FileType {
    pub fn parse(s: &str) -> Option<FileType> {
        match s.trim().to_lowercase().as_str() {
            "relative" => Some(FileType::Relative),
            "global" => Some(FileType::Global),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            FileType::Relative => "Relative",
            FileType::Global => "Global",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub name: String,
    pub version: String,
    pub creator: String,
    pub file_type: FileType,
    pub exe_name: Option<String>,
    pub exe_icon: Option<String>,

    pub exe_windowed: bool,
    pub steam_app_id: Option<u32>,
    pub compress_scripts: bool,
    pub compress_assets: bool,
    pub shard_assets: bool,
    pub compile_bytecode: bool,
    pub include: Vec<String>,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            version: String::new(),
            creator: String::new(),
            file_type: FileType::default(),
            exe_name: None,
            exe_icon: None,
            exe_windowed: true,
            steam_app_id: None,
            compress_scripts: false,
            compress_assets: false,
            shard_assets: false,
            compile_bytecode: false,
            include: Vec::new(),
        }
    }
}

fn parse_include(v: &toml::Value) -> Vec<String> {
    let mut out = Vec::new();
    let candidates: [Option<&toml::Value>; 5] = [
        v.get("include"),
        v.get("build").and_then(|b| b.get("include")),
        v.get("exe").and_then(|e| e.get("include")),
        v.get("steam").and_then(|s| s.get("include")),
        v.get("configs").and_then(|c| c.get("include")),
    ];
    for slot in candidates.iter().filter_map(|c| *c) {
        if let Some(arr) = slot.as_array() {
            for item in arr {
                if let Some(s) = item.as_str() {
                    out.push(s.to_string());
                }
            }
            if !out.is_empty() {
                return out;
            }
        }
        if let Some(t) = slot.as_table() {
            if let Some(arr) = t.get("paths").and_then(|x| x.as_array()) {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        out.push(s.to_string());
                    }
                }
                if !out.is_empty() {
                    return out;
                }
            }
        }
    }
    out
}

impl BuildConfig {
    pub fn from_toml_str(text: &str) -> Result<Self, String> {
        let v: toml::Value = text
            .parse::<toml::Value>()
            .map_err(|e| format!("invalid build.toml: {e}"))?;

        let mut cfg = BuildConfig::default();
        if let Some(s) = v.get("Name").and_then(|x| x.as_str()) {
            cfg.name = s.to_string();
        }
        if let Some(s) = v.get("Version").and_then(|x| x.as_str()) {
            cfg.version = s.to_string();
        }
        if let Some(s) = v.get("Creator").and_then(|x| x.as_str()) {
            cfg.creator = s.to_string();
        }
        if let Some(configs) = v.get("configs") {
            if let Some(ft_str) = configs.get("File Type").and_then(|x| x.as_str()) {
                cfg.file_type = FileType::parse(ft_str).ok_or_else(|| {
                    format!("File Type must be 'Relative' or 'Global', got '{ft_str}'")
                })?;
            }
        }
        if let Some(exe) = v.get("exe") {
            if let Some(s) = exe.get("name").and_then(|x| x.as_str()) {
                cfg.exe_name = Some(s.to_string());
            }
            if let Some(s) = exe.get("icon").and_then(|x| x.as_str()) {
                cfg.exe_icon = Some(s.to_string());
            }
            if let Some(b) = exe.get("windowed").and_then(|x| x.as_bool()) {
                cfg.exe_windowed = b;
            }
            if let Some(b) = exe.get("compress").and_then(|x| x.as_bool()) {
                cfg.compress_scripts = b;
                cfg.compress_assets = b;
            }
            if let Some(b) = exe.get("compress_scripts").and_then(|x| x.as_bool()) {
                cfg.compress_scripts = b;
            }
            if let Some(b) = exe.get("compress_assets").and_then(|x| x.as_bool()) {
                cfg.compress_assets = b;
            }
            if let Some(b) = exe.get("shard_assets").and_then(|x| x.as_bool()) {
                cfg.shard_assets = b;
            }
            if let Some(b) = exe.get("bytecode").and_then(|x| x.as_bool()) {
                cfg.compile_bytecode = b;
            }
            if let Some(b) = exe.get("compile_bytecode").and_then(|x| x.as_bool()) {
                cfg.compile_bytecode = b;
            }
        }
        if let Some(steam) = v.get("steam") {
            if let Some(id) = steam.get("app_id").and_then(|x| x.as_integer()) {
                if id > 0 && id <= u32::MAX as i64 {
                    cfg.steam_app_id = Some(id as u32);
                }
            }
        }
        cfg.include = parse_include(&v);
        Ok(cfg)
    }

    pub fn load(root: &Path) -> Result<Self, String> {
        let path = root.join("build.toml");
        if !path.is_file() {
            return Ok(BuildConfig::default());
        }
        let text =
            fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        Self::from_toml_str(&text)
    }

    pub fn print_banner(&self) {
        let mut parts: Vec<String> = Vec::new();
        if !self.name.is_empty() {
            parts.push(self.name.clone());
        }
        if !self.version.is_empty() {
            parts.push(format!("v{}", self.version));
        }
        if !self.creator.is_empty() {
            parts.push(format!("by {}", self.creator));
        }
        if !parts.is_empty() {
            println!(
                "[Ruzit] {}  (require mode: {})",
                parts.join(" "),
                self.file_type.as_str()
            );
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ManagedInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub creator: String,
    pub entry: String,
    pub file_type: FileType,
    pub include: Vec<String>,
}

impl ManagedInfo {
    pub fn from_toml_str(text: &str) -> Result<Self, String> {
        let v: toml::Value = text
            .parse::<toml::Value>()
            .map_err(|e| format!("invalid ManagedInfo.toml: {e}"))?;
        let id = v
            .get("ID")
            .and_then(|x| x.as_str())
            .ok_or("ManagedInfo.toml missing 'ID'")?
            .to_string();
        let mut info = ManagedInfo {
            id,
            entry: "init.luau".to_string(),
            ..Default::default()
        };
        if let Some(s) = v.get("Name").and_then(|x| x.as_str()) {
            info.name = s.to_string();
        } else {
            info.name = info.id.clone();
        }
        if let Some(s) = v.get("Version").and_then(|x| x.as_str()) {
            info.version = s.to_string();
        }
        if let Some(s) = v.get("Creator").and_then(|x| x.as_str()) {
            info.creator = s.to_string();
        }
        if let Some(s) = v.get("Entry").and_then(|x| x.as_str()) {
            info.entry = s.to_string();
        }
        if let Some(configs) = v.get("configs") {
            if let Some(ft_str) = configs.get("File Type").and_then(|x| x.as_str()) {
                info.file_type = FileType::parse(ft_str).ok_or_else(|| {
                    format!("File Type must be 'Relative' or 'Global', got '{ft_str}'")
                })?;
            }
        }
        info.include = parse_include(&v);
        Ok(info)
    }

    pub fn load(folder: &std::path::Path) -> Result<Self, String> {
        let path = folder.join("ManagedInfo.toml");
        if !path.is_file() {
            return Err(format!(
                "ManagedInfo.toml not found in {}",
                folder.display()
            ));
        }
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        Self::from_toml_str(&text)
    }
}
