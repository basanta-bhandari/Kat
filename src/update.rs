//! Check the project's main branch without changing the local checkout.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

pub const SSH_REPOSITORY: &str = "git@github.com:basanta-bhandari/Kat.git";
pub const INSTALLED_REVISION: &str = env!("FORGETUI_REVISION");

pub fn check() -> Result<String, String> {
    let latest = latest_revision()?;
    if latest == INSTALLED_REVISION {
        Ok(format!("Up to date: {}", &latest[..8]))
    } else {
        let current = INSTALLED_REVISION.get(..8).unwrap_or(INSTALLED_REVISION);
        Ok(format!(
            "Installed {current}; main branch {}. Choose Update to install.",
            &latest[..8]
        ))
    }
}

pub fn latest_revision() -> Result<String, String> {
    let output = Command::new("git")
        .args(["ls-remote", "--heads", SSH_REPOSITORY, "refs/heads/main"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=8",
        )
        .output()
        .map_err(|error| format!("Could not run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "SSH update check failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let response = String::from_utf8_lossy(&output.stdout);
    parse_revision(&response).ok_or_else(|| "Main branch was not found".into())
}

/// Clone and build in a temporary directory, then replace only the running executable.
pub fn install() -> Result<String, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("Cannot locate executable: {error}"))?;
    install_to(&executable)
}

fn install_to(executable: &Path) -> Result<String, String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let work = std::env::temp_dir().join(format!("forgetui-update-{}-{nonce}", std::process::id()));
    fs::create_dir(&work).map_err(|error| format!("Cannot create update directory: {error}"))?;
    let result = build_and_install(&work, executable, nonce);
    let _ = fs::remove_dir_all(&work);
    result
}

fn build_and_install(work: &Path, executable: &Path, nonce: u128) -> Result<String, String> {
    let checkout = work.join("source");
    let output = Command::new("git")
        .args(["clone", "--depth", "1", "--branch", "main", SSH_REPOSITORY])
        .arg(&checkout)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=8",
        )
        .output()
        .map_err(|error| format!("Could not run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Update download failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let output = Command::new("cargo")
        .args(["build", "--release", "--locked"])
        .current_dir(&checkout)
        .output()
        .map_err(|error| format!("Could not run cargo: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Update build failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let built = checkout
        .join("target")
        .join("release")
        .join(if cfg!(windows) {
            "forgetui.exe"
        } else {
            "forgetui"
        });
    let mut staged = PathBuf::from(executable);
    staged.set_extension(format!("update-{nonce}"));
    if staged.exists() {
        return Err("Temporary update file already exists".into());
    }
    fs::copy(&built, &staged).map_err(|error| {
        format!(
            "Cannot stage update next to {}: {error}",
            executable.display()
        )
    })?;
    if let Err(error) = fs::rename(&staged, executable) {
        let _ = fs::remove_file(&staged);
        return Err(format!("Cannot replace {}: {error}", executable.display()));
    }
    Ok("Update installed. Restart ForgeTUI to use it.".into())
}

fn parse_revision(response: &str) -> Option<String> {
    let (revision, reference) = response.lines().next()?.split_once('\t')?;
    (reference == "refs/heads/main"
        && revision.len() == 40
        && revision.bytes().all(|byte| byte.is_ascii_hexdigit()))
    .then(|| revision.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_a_main_branch_revision() {
        assert!(
            parse_revision("0123456789012345678901234567890123456789\trefs/heads/main\n").is_some()
        );
        assert!(parse_revision("bad\trefs/heads/main\n").is_none());
        assert!(
            parse_revision("0123456789012345678901234567890123456789\trefs/heads/other\n")
                .is_none()
        );
    }
}
