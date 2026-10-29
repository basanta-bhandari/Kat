//! Check the project's main branch without changing the local checkout.
use std::process::Command;

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
