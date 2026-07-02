//! Secure API-key storage via the OS credential store (`keyring`).
//! Keys are keyed by provider and never written to the project DB or config.

use anyhow::Result;

const SERVICE: &str = "rpgtl";

fn entry(provider: &str) -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(SERVICE, provider)?)
}

/// Store (or replace) the API key for a provider.
pub fn set_key(provider: &str, key: &str) -> Result<()> {
    entry(provider)?.set_password(key)?;
    Ok(())
}

/// Fetch the API key for a provider, if one is stored.
pub fn get_key(provider: &str) -> Result<Option<String>> {
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
