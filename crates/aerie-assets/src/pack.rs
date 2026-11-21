//! An installation's `Assets.zip`.

use std::{fs::File, io::Read, path::Path, sync::Mutex};

use crate::Result;

pub struct Pack {
    zip: Mutex<zip::ZipArchive<File>>,
    names: Vec<String>,
}

impl Pack {
    pub fn open(path: &Path) -> Result<Self> {
        let zip = zip::ZipArchive::new(File::open(path)?)?;
        let names = zip.file_names().map(str::to_owned).collect();
        Ok(Self {
            zip: Mutex::new(zip),
            names,
        })
    }

    /// The usual place of `Assets.zip` in a Windows installation, if present.
    pub fn default_path() -> Option<std::path::PathBuf> {
        let appdata = std::env::var_os("APPDATA")?;
        let p = Path::new(&appdata).join("Hytale/install/release/package/game/latest/Assets.zip");
        p.exists().then_some(p)
    }

    /// Entry names under `prefix` (a folder, without trailing slash) ending in `ext`.
    pub fn list(&self, prefix: &str, ext: &str) -> Vec<&str> {
        let prefix = format!("{prefix}/");
        self.names
            .iter()
            .filter(|n| n.starts_with(&prefix) && n.ends_with(ext))
            .map(String::as_str)
            .collect()
    }

    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let mut zip = self.zip.lock().unwrap();
        let mut f = zip.by_name(name)?;
        let mut v = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut v)?;
        Ok(v)
    }
}
