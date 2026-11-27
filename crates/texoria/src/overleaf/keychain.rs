//! Where Overleaf tokens are kept: the system keychain, one entry per server.
//!
//! GPUI's credentials API names its entries after Zed on Linux (the label `zed-github-account`)
//! and Windows (the target `zed:url=<server>`), which tells the user nothing about TeXoria. There
//! TeXoria writes its own entries, named by `entry_name`, and moves a token it finds under
//! GPUI's name (TeXoria 0.8.0 and earlier) the first time it reads it. On macOS GPUI's entry is an
//! Internet password named after the server, which is kept.

use std::rc::Rc;

use anyhow::Result;
use gpui_kit::*;
use tex_git::overleaf;

/// The name of a server's entry, as the system's credential manager shows it.
#[cfg(any(target_os = "linux", windows))]
fn entry_name(server: &str) -> String {
    format!("TeXoria Overleaf token ({server})")
}

/// A place that keeps a token per server.
pub(super) trait Store {
    fn read(&self, server: &str, cx: &App) -> Task<Result<Option<String>>>;
    fn write(&self, server: &str, token: &str, cx: &App) -> Task<Result<()>>;
    fn delete(&self, server: &str, cx: &App) -> Task<Result<()>>;
}

/// Where tokens go, and where older versions put them.
#[derive(Clone)]
pub(super) struct Keychain {
    pub current: Rc<dyn Store>,
    pub legacy: Option<Rc<dyn Store>>,
}

impl Global for Keychain {}

impl Default for Keychain {
    fn default() -> Self {
        // Tests never touch the user's keychain.
        if cfg!(test) {
            return Self {
                current: Rc::new(Memory::default()),
                legacy: None,
            };
        }
        system()
    }
}

#[cfg(any(target_os = "linux", windows))]
fn system() -> Keychain {
    Keychain {
        current: Rc::new(system::System),
        legacy: Some(Rc::new(Gpui)),
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
fn system() -> Keychain {
    Keychain {
        current: Rc::new(Gpui),
        legacy: None,
    }
}

/// The token kept for `server`, moved first from where an older version put it.
pub(super) fn read(server: &str, cx: &mut App) -> Task<Result<Option<String>>> {
    let Keychain { current, legacy } = cx.default_global::<Keychain>().clone();
    let server = server.to_string();
    cx.spawn(async move |cx| {
        let found = cx.update(|cx| current.read(&server, cx)).await?;
        if found.is_some() {
            return Ok(found);
        }
        let Some(legacy) = legacy else {
            return Ok(None);
        };
        let Some(token) = cx
            .update(|cx| legacy.read(&server, cx))
            .await?
            .filter(|t| overleaf::valid_token(t))
        else {
            return Ok(None);
        };
        match cx.update(|cx| current.write(&server, &token, cx)).await {
            Ok(()) => {
                if let Err(e) = cx.update(|cx| legacy.delete(&server, cx)).await {
                    log::warn!("deleting the old entry of the token for {server}: {e:#}");
                }
            }
            Err(e) => log::warn!("moving the token for {server}: {e:#}"),
        }
        Ok(Some(token))
    })
}

pub(super) fn write(server: &str, token: &str, cx: &mut App) -> Task<Result<()>> {
    let current = cx.default_global::<Keychain>().current.clone();
    current.write(server, token, cx)
}

/// GPUI's credentials API.
struct Gpui;

impl Store for Gpui {
    fn read(&self, server: &str, cx: &App) -> Task<Result<Option<String>>> {
        let read = cx.read_credentials(server);
        cx.background_executor().spawn(async move {
            Ok(read
                .await?
                .and_then(|(_, secret)| String::from_utf8(secret).ok()))
        })
    }

    fn write(&self, server: &str, token: &str, cx: &App) -> Task<Result<()>> {
        cx.write_credentials(server, overleaf::USERNAME, token.as_bytes())
    }

    fn delete(&self, server: &str, cx: &App) -> Task<Result<()>> {
        cx.delete_credentials(server)
    }
}

/// Tokens in memory, for tests; `failing` makes every call fail.
#[derive(Clone, Default)]
pub(super) struct Memory {
    pub tokens: Rc<std::cell::RefCell<std::collections::HashMap<String, String>>>,
    pub failing: bool,
}

impl Memory {
    fn check(&self) -> Result<()> {
        anyhow::ensure!(!self.failing, "no keychain");
        Ok(())
    }
}

impl Store for Memory {
    fn read(&self, server: &str, _: &App) -> Task<Result<Option<String>>> {
        Task::ready(
            self.check()
                .map(|()| self.tokens.borrow().get(server).cloned()),
        )
    }

    fn write(&self, server: &str, token: &str, _: &App) -> Task<Result<()>> {
        Task::ready(self.check().map(|()| {
            self.tokens
                .borrow_mut()
                .insert(server.to_string(), token.to_string());
        }))
    }

    fn delete(&self, server: &str, _: &App) -> Task<Result<()>> {
        Task::ready(self.check().map(|()| {
            self.tokens.borrow_mut().remove(server);
        }))
    }
}

/// The Secret Service (GNOME Keyring, KWallet…): an item labeled [`entry_name`], with the
/// attributes `application` = `texoria`, `url` = the server and `username`.
#[cfg(target_os = "linux")]
mod system {
    use super::*;

    pub struct System;

    const APPLICATION: (&str, &str) = ("application", "texoria");

    async fn keyring() -> Result<oo7::Keyring> {
        let keyring = oo7::Keyring::new().await?;
        keyring.unlock().await?;
        Ok(keyring)
    }

    impl Store for System {
        fn read(&self, server: &str, cx: &App) -> Task<Result<Option<String>>> {
            let server = server.to_string();
            cx.background_executor().spawn(async move {
                let keyring = keyring().await?;
                let items = keyring
                    .search_items(&[APPLICATION, ("url", &server)])
                    .await?;
                let Some(item) = items.into_iter().next() else {
                    return Ok(None);
                };
                item.unlock().await?;
                let secret = item.secret().await?;
                Ok(String::from_utf8(secret.as_bytes().to_vec()).ok())
            })
        }

        fn write(&self, server: &str, token: &str, cx: &App) -> Task<Result<()>> {
            let server = server.to_string();
            let token = oo7::Secret::text(token);
            cx.background_executor().spawn(async move {
                let attributes = [
                    APPLICATION,
                    ("url", &server),
                    ("username", overleaf::USERNAME),
                ];
                keyring()
                    .await?
                    .create_item(&entry_name(&server), &attributes, token, true)
                    .await?;
                Ok(())
            })
        }

        fn delete(&self, server: &str, cx: &App) -> Task<Result<()>> {
            let server = server.to_string();
            cx.background_executor().spawn(async move {
                keyring()
                    .await?
                    .delete(&[APPLICATION, ("url", &server)])
                    .await?;
                Ok(())
            })
        }
    }
}

/// The Credential Manager: a generic credential whose target is [`entry_name`], for the user
/// `git`, kept on this machine.
#[cfg(windows)]
mod system {
    use std::ffi::c_void;

    use windows::Win32::Foundation::ERROR_NOT_FOUND;
    use windows::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
        CredReadW, CredWriteW,
    };
    use windows::core::{PCWSTR, PWSTR};

    use super::*;

    pub struct System;

    /// A NUL-terminated UTF-16 string.
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn not_found(e: &windows::core::Error) -> bool {
        e.code() == ERROR_NOT_FOUND.to_hresult()
    }

    fn read(target: &[u16]) -> Result<Option<String>> {
        let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `target` is NUL-terminated and `credential` receives a buffer that is freed
        // below.
        let read = unsafe {
            CredReadW(
                PCWSTR(target.as_ptr()),
                CRED_TYPE_GENERIC,
                None,
                &mut credential,
            )
        };
        match read {
            Err(e) if not_found(&e) => return Ok(None),
            Err(e) => return Err(e.into()),
            Ok(()) if credential.is_null() => return Ok(None),
            Ok(()) => {}
        }
        // SAFETY: `CredReadW` succeeded, so `credential` points to a valid `CREDENTIALW`, whose
        // blob is `CredentialBlobSize` bytes long, until `CredFree`.
        let secret = unsafe {
            let c = &*credential;
            let secret = if c.CredentialBlob.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec()
            };
            CredFree(credential as *const c_void);
            secret
        };
        Ok(String::from_utf8(secret).ok())
    }

    fn write(target: &mut [u16], token: &str) -> Result<()> {
        let mut username = wide(overleaf::USERNAME);
        let mut secret = token.as_bytes().to_vec();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_mut_ptr()),
            CredentialBlobSize: u32::try_from(secret.len())?,
            CredentialBlob: secret.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(username.as_mut_ptr()),
            ..CREDENTIALW::default()
        };
        // SAFETY: every pointer in `credential` is valid for the call.
        unsafe { CredWriteW(&credential, 0)? };
        Ok(())
    }

    fn delete(target: &[u16]) -> Result<()> {
        // SAFETY: `target` is NUL-terminated.
        match unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Err(e) if !not_found(&e) => Err(e.into()),
            _ => Ok(()),
        }
    }

    impl Store for System {
        fn read(&self, server: &str, cx: &App) -> Task<Result<Option<String>>> {
            let target = wide(&entry_name(server));
            cx.background_executor().spawn(async move { read(&target) })
        }

        fn write(&self, server: &str, token: &str, cx: &App) -> Task<Result<()>> {
            let mut target = wide(&entry_name(server));
            let token = token.to_string();
            cx.background_executor()
                .spawn(async move { write(&mut target, &token) })
        }

        fn delete(&self, server: &str, cx: &App) -> Task<Result<()>> {
            let target = wide(&entry_name(server));
            cx.background_executor()
                .spawn(async move { delete(&target) })
        }
    }
}
