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

#[derive(Debug, Clone, Default)]
pub struct BuildConfig {
    pub name: String,
    pub version: String,
    pub creator: String,
    pub file_type: FileType,
    pub exe_name: Option<String>,
    pub exe_icon: Option<String>,
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
        }
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
