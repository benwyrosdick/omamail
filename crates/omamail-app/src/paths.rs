use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl AppPaths {
    pub fn system() -> Self {
        if let Some(root) = std::env::var_os("OMAMAIL_ROOT") {
            return Self::for_root(Path::new(&root));
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        Self {
            config_dir: config_home.join("omamail"),
            data_dir: data_home.join("omamail"),
        }
    }

    pub fn for_root(root: &Path) -> Self {
        Self {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
        }
    }

    pub fn database(&self) -> PathBuf {
        self.data_dir.join("omamail.db")
    }

    pub fn oauth_file(&self) -> PathBuf {
        self.config_dir.join("oauth.toml")
    }
}
