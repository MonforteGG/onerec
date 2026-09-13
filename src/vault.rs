pub(crate) const TARGET: &str = "onerec/api_key";

pub(crate) trait SecretStore: 'static {
    fn load(&self) -> Result<Option<String>, String>;
    fn save(&self, key: &str) -> Result<(), String>;
    fn delete(&self) -> Result<(), String>;
}

#[cfg(windows)]
pub(crate) struct CredentialManager;

#[cfg(windows)]
impl SecretStore for CredentialManager {
    fn load(&self) -> Result<Option<String>, String> {
        cred_load()
    }

    fn save(&self, key: &str) -> Result<(), String> {
        cred_save(key)
    }

    fn delete(&self) -> Result<(), String> {
        cred_delete()
    }
}

pub(crate) fn adopt_plaintext(
    vault: &dyn SecretStore,
    leftover: Option<&str>,
) -> Result<bool, String> {
    let Some(key) = leftover.map(str::trim).filter(|key| !key.is_empty()) else {
        return Ok(false);
    };
    vault.save(key)?;
    Ok(true)
}

#[cfg(windows)]
fn cred_load() -> Result<Option<String>, String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_NOT_FOUND;
    use windows::Win32::Security::Credentials::{
        CredFree, CredReadW, CRED_TYPE_GENERIC,
    };

    unsafe {
        let mut credential = std::ptr::null_mut();
        let name = target_wide();
        match CredReadW(
            PCWSTR(name.as_ptr()),
            CRED_TYPE_GENERIC,
            0,
            &mut credential,
        ) {
            Ok(()) => {
                let result = read_blob(credential);
                CredFree(credential.cast());
                result
            }
            Err(error) if error.code() == ERROR_NOT_FOUND.to_hresult() => Ok(None),
            Err(_) => Err("Could not read the API key from Credential Manager.".into()),
        }
    }
}

#[cfg(windows)]
fn read_blob(credential: *const windows::Win32::Security::Credentials::CREDENTIALW) -> Result<Option<String>, String> {
    if credential.is_null() {
        return Ok(None);
    }
    let credential = unsafe { &*credential };
    if credential.CredentialBlob.is_null() || credential.CredentialBlobSize == 0 {
        return Ok(None);
    }
    let bytes = unsafe {
        std::slice::from_raw_parts(
            credential.CredentialBlob,
            credential.CredentialBlobSize as usize,
        )
    };
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "The stored API key is not valid UTF-8.".to_string())?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_owned()))
    }
}

#[cfg(windows)]
fn target_wide() -> Vec<u16> {
    TARGET.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn cred_save(key: &str) -> Result<(), String> {
    use windows::core::PWSTR;
    use windows::Win32::Security::Credentials::{
        CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    let mut blob = key.as_bytes().to_vec();
    let mut name = target_wide();
    let mut user: Vec<u16> = "onerec".encode_utf16().chain(std::iter::once(0)).collect();
    let mut credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(name.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user.as_mut_ptr()),
        ..Default::default()
    };
    unsafe { CredWriteW(&mut credential, 0) }
        .map_err(|_| "Could not save the API key in Credential Manager.".to_string())
}

#[cfg(windows)]
fn cred_delete() -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_NOT_FOUND;
    use windows::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};

    let name = target_wide();
    match unsafe { CredDeleteW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, 0) } {
        Ok(()) => Ok(()),
        Err(error) if error.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
        Err(_) => Err("Could not delete the API key from Credential Manager.".into()),
    }
}

#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct MemoryVault {
    key: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

#[cfg(test)]
impl MemoryVault {
    pub(crate) fn from_key(key: impl Into<String>) -> Self {
        let key = key.into();
        let trimmed = key.trim();
        Self {
            key: std::sync::Arc::new(std::sync::Mutex::new(
                (!trimmed.is_empty()).then(|| trimmed.to_owned()),
            )),
        }
    }
}

#[cfg(test)]
impl SecretStore for MemoryVault {
    fn load(&self) -> Result<Option<String>, String> {
        Ok(self.key.lock().expect("vault").clone())
    }

    fn save(&self, key: &str) -> Result<(), String> {
        *self.key.lock().expect("vault") = Some(key.to_owned());
        Ok(())
    }

    fn delete(&self) -> Result<(), String> {
        *self.key.lock().expect("vault") = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_vault_round_trips_and_delete_is_idempotent() {
        let vault = MemoryVault::default();
        assert_eq!(vault.load().unwrap(), None);
        vault.save("gsk_live").unwrap();
        assert_eq!(vault.load().unwrap().as_deref(), Some("gsk_live"));
        vault.save("gsk_other").unwrap();
        assert_eq!(vault.load().unwrap().as_deref(), Some("gsk_other"));
        vault.delete().unwrap();
        assert_eq!(vault.load().unwrap(), None);
        vault.delete().unwrap();
        assert_eq!(MemoryVault::from_key("  gsk_trim  ").load().unwrap().as_deref(), Some("gsk_trim"));
        assert_eq!(MemoryVault::from_key("   ").load().unwrap(), None);
    }

    #[test]
    fn adopt_plaintext_saves_leftover_key_and_skips_blanks() {
        let vault = MemoryVault::default();
        assert!(!adopt_plaintext(&vault, None).unwrap());
        assert!(!adopt_plaintext(&vault, Some("")).unwrap());
        assert!(!adopt_plaintext(&vault, Some("   ")).unwrap());
        assert_eq!(vault.load().unwrap(), None);
        assert!(adopt_plaintext(&vault, Some(" gsk_legacy ")).unwrap());
        assert_eq!(vault.load().unwrap().as_deref(), Some("gsk_legacy"));
        assert!(adopt_plaintext(&vault, Some("gsk_upgrade")).unwrap());
        assert_eq!(vault.load().unwrap().as_deref(), Some("gsk_upgrade"));
    }
}
