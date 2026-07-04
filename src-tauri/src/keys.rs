//! Secure API-key storage via the OS credential store (`keyring`).
//! Keys are keyed by provider and never written to the project DB or config.
//!
//! For local development a key may instead come from an environment variable so
//! `pnpm tauri dev` can run against a shell-exported key without touching the OS
//! keychain — see [`env_key`]. This fallback is compiled in **debug builds only**;
//! release builds read the keychain exclusively, keeping the secrets-vs-config
//! split intact.

use anyhow::Result;

const SERVICE: &str = "rpgtl";

fn entry(provider: &str) -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(SERVICE, provider)?)
}

/// Dev-only environment-variable source for an API key. Checks
/// `RPGTL_KEY_<KIND>` (e.g. `RPGTL_KEY_OPENAI`) first, then the provider's
/// conventional variable (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, …). Returns the
/// trimmed value, or `None` when unset/blank. Present only in debug builds.
#[cfg(debug_assertions)]
fn env_key(provider: &str) -> Option<String> {
    let specific = format!("RPGTL_KEY_{}", provider.to_ascii_uppercase());
    let conventional = match provider {
        "openai" => Some("OPENAI_API_KEY"),
        "openrouter" => Some("OPENROUTER_API_KEY"),
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "gemini" => Some("GEMINI_API_KEY"),
        _ => None,
    };
    std::env::var(&specific)
        .ok()
        .or_else(|| conventional.and_then(|name| std::env::var(name).ok()))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Release builds never read keys from the environment.
#[cfg(not(debug_assertions))]
fn env_key(_provider: &str) -> Option<String> {
    None
}

/// Store (or replace) the API key for a provider.
pub fn set_key(provider: &str, key: &str) -> Result<()> {
    entry(provider)?.set_password(key)?;
    Ok(())
}

/// Fetch the API key for a provider, if one is available. In debug builds a
/// matching environment variable takes precedence over the keychain so a dev key
/// can be supplied from the shell.
pub fn get_key(provider: &str) -> Result<Option<String>> {
    if let Some(k) = env_key(provider) {
        return Ok(Some(k));
    }
    match entry(provider)?.get_password() {
        Ok(k) => Ok(Some(k)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// True if a key is stored (without revealing it).
pub fn has_key(provider: &str) -> Result<bool> {
    Ok(get_key(provider)?.is_some())
}

/// Remove the stored key for a provider.
pub fn delete_key(provider: &str) -> Result<()> {
    match entry(provider)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;

    #[test]
    fn env_key_reads_specific_var_and_trims() {
        // A synthetic provider with no conventional-name mapping and no keychain
        // entry, so this exercises only the `RPGTL_KEY_<KIND>` env path.
        let var = "RPGTL_KEY_ENVTESTPROVIDER";
        std::env::set_var(var, "  sk-dev-123  ");
        assert_eq!(env_key("envtestprovider").as_deref(), Some("sk-dev-123"));
        std::env::set_var(var, "   ");
        assert_eq!(env_key("envtestprovider"), None, "blank is treated as unset");
        std::env::remove_var(var);
        assert_eq!(env_key("envtestprovider"), None);
    }
}
