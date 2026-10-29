use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct ProjectEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_directory: bool,
}

pub fn browse(directory: &Path) -> io::Result<Vec<ProjectEntry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let is_directory = path.is_dir();
        let supported = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "forge" | "stl" | "3mf" | "step" | "stp"
            )
        });
        if is_directory || supported {
            entries.push(ProjectEntry {
                path,
                name: entry.file_name().to_string_lossy().into_owned(),
                is_directory,
            });
        }
    }
    entries.sort_by_key(|entry| (!entry.is_directory, entry.name.to_lowercase()));
    Ok(entries)
}
