//! The SDK's table of contents: what it can answer for, and where each answer is stored.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The version of this file's layout. A wrapper refuses an SDK of another one.
pub const FORMAT: u32 = 1;

/// The file name of the manifest inside an SDK directory.
pub const FILE: &str = "manifest.json";

/// A file the wrapper copies into rustc's output directory as it is.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Copy {
    /// Where it is stored, relative to the SDK directory.
    pub stored: String,
    /// The file name rustc would have written.
    pub name: String,
}

/// A crate compiled as a library (`rlib`): answered with a stub rlib and the metadata cut from it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stub {
    /// The rlib (whole, or a metadata-only stub), relative to the SDK directory.
    pub stored: String,
    /// Where the plain `.rmeta` file starts inside a stub, and how long it is.
    pub rmeta_offset: u64,
    pub rmeta_len: u64,
    /// The plain `.rmeta` file, when the rlib is stored whole.
    #[serde(default)]
    pub rmeta_stored: Option<String>,
}

/// One compile unit the SDK can answer for.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Unit {
    /// The crate's name as rustc sees it (`--crate-name`).
    pub crate_name: String,
    /// What Cargo appends to every output name (`-C extra-filename=`): `-` and the metadata hash. Two
    /// builds of one crate with other features, flags or dependencies have different values here, which
    /// is what makes it safe to answer by this key.
    pub extra: String,
    pub crate_types: Vec<String>,
    /// For a shared library: what it was compiled against, as `ident_of_path` names the files Cargo passed with
    /// `--extern`, sorted. A shared library's own output names carry no hash, so this is what says it is the same one.
    #[serde(default)]
    pub externs: Vec<String>,
    pub stub: Option<Stub>,
    /// Outputs copied verbatim: a shared library and its import library.
    pub copy: Vec<Copy>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    /// `rustc -vV` of the build the SDK was cut from: an SDK serves only the same compiler.
    pub rustc: String,
    pub units: Vec<Unit>,
}

impl Manifest {
    pub fn read(dir: &Path) -> Result<Self, String> {
        let path = dir.join(FILE);
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let manifest: Manifest =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if manifest.format != FORMAT {
            return Err(format!(
                "{}: SDK format {} (this build reads {FORMAT})",
                path.display(),
                manifest.format
            ));
        }
        Ok(manifest)
    }

    pub fn write(&self, dir: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(dir.join(FILE), text).map_err(|e| e.to_string())
    }
}

/// Every unit of the SDK directories in `SDK_ENV`, by `(crate name, extra filename)`.
pub struct Index {
    units: HashMap<(String, String), Vec<(PathBuf, Unit)>>,
    pub rustc: Option<String>,
}

impl Index {
    pub fn load(dirs: &[PathBuf]) -> Self {
        let mut units: HashMap<(String, String), Vec<(PathBuf, Unit)>> = HashMap::new();
        let mut rustc = None;
        for dir in dirs {
            let Ok(manifest) = Manifest::read(dir) else {
                continue;
            };
            rustc.get_or_insert(manifest.rustc.clone());
            for unit in manifest.units {
                units
                    .entry((unit.crate_name.clone(), unit.extra.clone()))
                    .or_default()
                    .push((dir.clone(), unit));
            }
        }
        Self { units, rustc }
    }

    /// The units answering to this name and extra filename: one for a library crate (the hash is in the key),
    /// possibly several for a shared library (no hash: see `Unit::externs`).
    pub fn get(&self, crate_name: &str, extra: &str) -> &[(PathBuf, Unit)] {
        self.units
            .get(&(crate_name.to_owned(), extra.to_owned()))
            .map_or(&[], Vec::as_slice)
    }

    pub fn len(&self) -> usize {
        self.units.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }
}
