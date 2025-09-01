use std::fs;
use std::io;
use std::path::{Path, PathBuf};

fn sanitize_name(name: &str) -> Result<&str, io::Error> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid name"));
    }
    // Basic illegal character checks for cross-platform safety
    #[cfg(target_os = "windows")]
    {
        const ILLEGAL: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
        if name.chars().any(|c| ILLEGAL.contains(&c)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid characters",
            ));
        }
        // Reserved names on Windows
        let upper = name.to_ascii_uppercase();
        const RESERVED: [&str; 8] = ["CON", "PRN", "AUX", "NUL", "COM1", "LPT1", "COM2", "LPT2"];
        if RESERVED.iter().any(|r| r == &upper) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "reserved name"));
        }
    }
    Ok(name)
}

pub fn create_file(parent: &Path, name: &str) -> Result<PathBuf, io::Error> {
    let name = sanitize_name(name)?;
    let path = parent.join(name);
    if path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "already exists",
        ));
    }
    fs::create_dir_all(parent)?; // ensure parent exists
    fs::File::create(&path)?;
    Ok(path)
}

pub fn create_dir(parent: &Path, name: &str) -> Result<PathBuf, io::Error> {
    let name = sanitize_name(name)?;
    let path = parent.join(name);
    if path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "already exists",
        ));
    }
    fs::create_dir_all(&path)?;
    Ok(path)
}

pub fn rename_path(from: &Path, to_name: &str) -> Result<PathBuf, io::Error> {
    let to_name = sanitize_name(to_name)?;
    let parent = from
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent"))?;
    let to = parent.join(to_name);

    // Case-only rename on case-insensitive FS may fail; try two-step with temp
    let do_rename = || fs::rename(from, &to);
    match do_rename() {
        Ok(()) => Ok(to),
        Err(e) => {
            // Best-effort case-only rename support on case-insensitive FS:
            // if target differs only by case, use a two-step rename via a temp path.
            let from_name = from.file_name().and_then(|s| s.to_str());
            let to_name = to.file_name().and_then(|s| s.to_str());
            if let (Some(f), Some(t)) = (from_name, to_name) {
                if f.eq_ignore_ascii_case(t) && f != t {
                    let tmp = parent.join(format!(".tmp_rename_{}_{}", std::process::id(), t));
                    fs::rename(from, &tmp)?;
                    fs::rename(&tmp, &to)?;
                    return Ok(to);
                }
            }
            Err(e)
        }
    }
}

pub fn delete_path(path: &Path) -> Result<(), io::Error> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

pub fn duplicate_path(src: &Path, target_name: &str) -> Result<PathBuf, io::Error> {
    let target_name = sanitize_name(target_name)?;
    let parent = src
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent"))?;
    let dst = parent.join(target_name);
    if dst.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "target exists",
        ));
    }
    if src.is_dir() {
        copy_dir_recursive(src, &dst)?;
    } else {
        fs::copy(src, &dst)?;
    }
    Ok(dst)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), io::Error> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_file() {
            fs::copy(&from, &to)?;
        } else if file_type.is_symlink() {
            // Best-effort: copy link target contents (follow symlink)
            match fs::read_link(&from) {
                Ok(target) => {
                    let abs = if target.is_absolute() {
                        target
                    } else {
                        from.parent().unwrap_or(Path::new(".")).join(target)
                    };
                    if abs.is_dir() {
                        copy_dir_recursive(&abs, &to)?;
                    } else {
                        fs::copy(&abs, &to)?;
                    }
                }
                Err(_) => {
                    // skip unreadable symlink
                }
            }
        }
    }
    Ok(())
}
