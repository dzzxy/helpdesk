use crate::{model::{Credentials, Result}, win};
use std::ptr::null_mut;
use windows_sys::Win32::{Foundation::ERROR_NOT_FOUND, Security::Credentials::*};
use zeroize::{Zeroize, Zeroizing};

const PREFIX: &str = "CorpInstallerRust:";

pub fn accounts() -> Result<Vec<String>> {
    let mut names=accounts_prefix(PREFIX)?;
    names.extend(accounts_prefix("CorpInstaller/")?);
    names.sort_by_key(|name|name.to_lowercase());
    names.dedup_by(|a,b|a.eq_ignore_ascii_case(b));
    Ok(names)
}
fn accounts_prefix(prefix:&str) -> Result<Vec<String>> {
    let mut count = 0;
    let mut entries = null_mut();
    unsafe {
        if CredEnumerateW(win::wide(&format!("{prefix}*")).as_ptr(), 0, &mut count, &mut entries) == 0 {
            let error = std::io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_NOT_FOUND as i32) { Ok(Vec::new()) } else { Err(error.to_string()) };
        }
        let mut names: Vec<String> = std::slice::from_raw_parts(entries, count as usize).iter()
            .map(|&entry| win::string((*entry).UserName)).collect();
        CredFree(entries.cast());
        names.sort_by_key(|name| name.to_lowercase());
        Ok(names)
    }
}

pub fn read(account: &str) -> Result<String> {
    read_prefix(PREFIX,account).or_else(|_|read_prefix("CorpInstaller/",account))
}
fn read_prefix(prefix:&str,account:&str)->Result<String>{
    let mut entry = null_mut();
    unsafe {
        win::check(CredReadW(win::wide(&format!("{prefix}{account}")).as_ptr(), CRED_TYPE_GENERIC, 0, &mut entry), "读取保存的密码失败")?;
        let credential = &*entry;
        let result = if credential.CredentialBlobSize == 0 || credential.CredentialBlobSize % 2 != 0 || credential.CredentialBlob.is_null() {
            Err("保存的密码格式无效，请重新输入。".into())
        } else {
            let bytes = std::slice::from_raw_parts_mut(credential.CredentialBlob, credential.CredentialBlobSize as usize);
            let words = Zeroizing::new(bytes.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect::<Vec<_>>());
            bytes.zeroize();
            String::from_utf16(&words).map_err(|e| e.to_string())
        };
        CredFree(entry.cast());
        result
    }
}

pub fn write(credentials: &Credentials) -> Result<()> {
    let mut secret = Zeroizing::new(credentials.password.encode_utf16().collect::<Vec<_>>());
    let mut target = win::wide(&format!("{PREFIX}{}", credentials.account));
    let mut account = win::wide(&credentials.account);
    let entry = CREDENTIALW {
        Type: CRED_TYPE_GENERIC, TargetName: target.as_mut_ptr(), UserName: account.as_mut_ptr(),
        CredentialBlob: secret.as_mut_ptr().cast(), CredentialBlobSize: (secret.len() * 2) as u32,
        Persist: CRED_PERSIST_LOCAL_MACHINE, ..Default::default()
    };
    unsafe { win::check(CredWriteW(&entry, 0), "保存密码失败") }
}

pub fn delete(account: &str) -> Result<()> {
    unsafe {
        let own=CredDeleteW(win::wide(&format!("{PREFIX}{account}")).as_ptr(), CRED_TYPE_GENERIC, 0)!=0;
        let legacy=CredDeleteW(win::wide(&format!("CorpInstaller/{account}")).as_ptr(), CRED_TYPE_GENERIC, 0)!=0;
        if own||legacy{Ok(())}else{Err(win::error("删除保存的账号失败"))}
    }
}

pub fn preferred()->Option<String>{
    let local=crate::tasks::data_directory().ok()?.join("credential-profile.json");
    let legacy=std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("CorpInstaller/credential-profile.json");
    [local,legacy].iter().find_map(|path|{
        let value:serde_json::Value=serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        value["Account"].as_str().map(str::to_owned)
    })
}
pub fn remember(account:&str)->Result<()>{
    let path=crate::tasks::data_directory()?.join("credential-profile.json");
    win::io(std::fs::write(path,serde_json::json!({"Account":account}).to_string()))
}
