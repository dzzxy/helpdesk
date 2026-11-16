use crate::{model::Result,win::{self,Handle,wide}};
use serde::{Deserialize,Serialize};
use std::{fs::File,io::{Read,Seek,SeekFrom},mem::zeroed,os::windows::io::{AsRawHandle,FromRawHandle},path::Path,ptr::{null,null_mut}};
use windows_sys::Win32::{Foundation::*,Security::{*,Authorization::*},Storage::FileSystem::*};

#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct Rule{
    pub id:String,pub name:String,pub target:String,pub target_sha256:String,
    pub arguments:String,pub working_directory:String,pub allowed_sids:Vec<String>,
    pub enabled:bool,pub trusted_application_confirmed:bool,
    #[serde(default)] pub created:serde_json::Value,
}

pub fn id(value:&str)->Result<String>{
    let normalized=value.replace('-',"");
    if !matches!(value.len(),32|36)||normalized.len()!=32||normalized.bytes().any(|b|!b.is_ascii_hexdigit())||normalized.bytes().all(|b|b==b'0'){
        return Err("INVALID_RULE: 规则编号无效。".into());
    }
    if value.len()==36 && [8,13,18,23].iter().any(|&i|value.as_bytes()[i]!=b'-'){return Err("INVALID_RULE: UUID 格式无效。".into());}
    Ok(normalized.to_lowercase())
}
pub fn local_path(path:&str)->Result<()>{
    crate::model::drive_path(path)?;
    if path.len()>240||path.len()<4||path.contains(['%','\r','\n'])||path.split('\\').any(|part|part.ends_with([' ','.'])){
        return Err("INVALID_TARGET: 需要规范的本地绝对路径。".into());
    }
    Ok(())
}
impl Rule{
    pub fn validate(&self)->Result<()>{
        id(&self.id)?;local_path(&self.target)?;local_path(&self.working_directory)?;
        let hosts=["cmd.exe","powershell.exe","pwsh.exe","wscript.exe","cscript.exe","mshta.exe","rundll32.exe","regsvr32.exe",
            "msiexec.exe","mmc.exe","reg.exe","sc.exe","schtasks.exe","wmic.exe","python.exe","pythonw.exe","node.exe",
            "bash.exe","wsl.exe","conhost.exe","wt.exe","explorer.exe","dllhost.exe","installutil.exe","msbuild.exe","csc.exe",
            "vbc.exe","dotnet.exe","runas.exe","taskmgr.exe"];
        let name=Path::new(&self.target).file_name().and_then(|name|name.to_str()).unwrap_or_default().to_ascii_lowercase();
        if !name.ends_with(".exe")||hosts.contains(&name.as_str()){return Err("EXECUTION_HOST_DENIED: 目标不能是通用执行宿主。".into());}
        if self.name.trim().is_empty()||self.name.chars().count()>100||self.allowed_sids.is_empty()||self.allowed_sids.len()>100{
            return Err("INVALID_RULE: 名称或允许用户无效。".into());
        }
        for sid in &self.allowed_sids{
            if !sid.starts_with("S-1-5-21-"){return Err("INVALID_RULE: 必须指定明确的用户 SID。".into());}
            unsafe{
                let mut parsed=null_mut();
                win::check(ConvertStringSidToSidW(wide(sid).as_ptr(),&mut parsed),"INVALID_RULE: 用户 SID 格式无效")?;
                LocalFree(parsed);
            }
        }
        if !self.trusted_application_confirmed{return Err("TRUST_NOT_CONFIRMED: 管理员尚未确认软件可信。".into());}
        if self.arguments.len()>4096||self.arguments.contains(['\0','\r','\n']){return Err("INVALID_ARGUMENTS: 参数无效。".into());}
        if !self.target_sha256.is_empty()&&(self.target_sha256.len()!=64||!self.target_sha256.bytes().all(|b|b.is_ascii_hexdigit())){
            return Err("INVALID_RULE: SHA-256 格式无效。".into());
        }
        Ok(())
    }
    pub fn authorize(&self,sid:&str,session:u32)->bool{
        self.enabled&&session>0&&self.allowed_sids.iter().any(|allowed|allowed.eq_ignore_ascii_case(sid))
    }
}

pub struct TargetLease{directories:Vec<Handle>,file:File}
impl TargetLease{
    pub fn open(rule:&Rule,check_hash:bool)->Result<Self>{
        rule.validate()?;
        let mut directories=Vec::new();let mut paths=Vec::new();
        for directory in [Path::new(&rule.target).parent().unwrap(),Path::new(&rule.working_directory)]{
            for parent in directory.ancestors(){if !paths.contains(&parent.to_path_buf()){paths.push(parent.to_path_buf());}}
        }
        paths.sort_by_key(|path|path.as_os_str().len());
        for path in paths{
            unsafe{
                let root=path.components().next().map(|p|p.as_os_str().to_string_lossy().to_string()+"\\").unwrap_or_default();
                if GetDriveTypeW(wide(&root).as_ptr())!=3{return Err("INVALID_TARGET: 需要本地固定磁盘。".into());}
                let handle=CreateFileW(wide(&path.to_string_lossy()).as_ptr(),FILE_READ_ATTRIBUTES,FILE_SHARE_READ|FILE_SHARE_WRITE,null(),OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS|FILE_FLAG_OPEN_REPARSE_POINT,null_mut());
                if handle==INVALID_HANDLE_VALUE{return Err(win::error("TARGET_LOCK_FAILED: 无法锁定目录"));}
                let handle=Handle(handle);verify_handle(handle.0,&path)?;
                directories.push(handle);
            }
        }
        let handle=unsafe{CreateFileW(wide(&rule.target).as_ptr(),GENERIC_READ,FILE_SHARE_READ,null(),OPEN_EXISTING,FILE_FLAG_OPEN_REPARSE_POINT,null_mut())};
        if handle==INVALID_HANDLE_VALUE{return Err(win::error("TARGET_LOCK_FAILED: 无法锁定主 EXE"));}
        let file=unsafe{File::from_raw_handle(handle)};
        verify_handle(handle,Path::new(&rule.target))?;
        let mut lease=Self{directories,file};
        if check_hash&&(rule.target_sha256.is_empty()||!lease.hash()?.eq_ignore_ascii_case(&rule.target_sha256)){
            return Err("TARGET_HASH_MISMATCH: 主 EXE 已变化，请重新授权。".into());
        }
        Ok(lease)
    }
    pub fn hash(&mut self)->Result<String>{
        use sha2::{Digest,Sha256};
        win::io(self.file.seek(SeekFrom::Start(0)))?;let mut hash=Sha256::new();let mut buffer=vec![0;1024*1024];
        loop{let count=win::io(self.file.read(&mut buffer))?;if count==0{break;}hash.update(&buffer[..count]);}
        Ok(format!("{:x}",hash.finalize()))
    }
    pub fn verify(&self,rule:&Rule)->Result<()>{
        let _=&self.directories;
        verify_handle(self.file.as_raw_handle(),Path::new(&rule.target))
    }
}
fn verify_handle(handle:HANDLE,expected:&Path)->Result<()>{
    unsafe{
        let mut info=zeroed();
        win::check(GetFileInformationByHandle(handle,&mut info),"读取目标文件标识失败")?;
        if info.dwFileAttributes&FILE_ATTRIBUTE_REPARSE_POINT!=0{return Err("REPARSE_POINT: 路径中有重解析点。".into());}
        let mut buffer=vec![0;32768];
        let length=GetFinalPathNameByHandleW(handle,buffer.as_mut_ptr(),buffer.len() as u32,0);
        if length==0||length as usize>=buffer.len(){return Err(win::error("无法解析文件实际路径"));}
        let final_path=String::from_utf16_lossy(&buffer[..length as usize]);
        let final_path=final_path.strip_prefix("\\\\?\\").unwrap_or(&final_path);
        if final_path.to_lowercase()!=expected.to_string_lossy().trim_end_matches('\\').to_lowercase()
            && final_path.trim_end_matches('\\').to_lowercase()!=expected.to_string_lossy().trim_end_matches('\\').to_lowercase(){
            return Err("PATH_REDIRECTED: 实际文件路径与授权目标不符。".into());
        }
        Ok(())
    }
}

pub fn inspect_storage(path:&Path)->Result<()>{
    use std::os::windows::fs::MetadataExt;
    let metadata=win::io(std::fs::symlink_metadata(path))?;
    if metadata.file_attributes()&FILE_ATTRIBUTE_REPARSE_POINT!=0{return Err("UNTRUSTED_STORAGE: 重解析点。".into());}
    unsafe{
        let mut owner=null_mut();let mut acl=null_mut();let mut descriptor=null_mut();
        let status=GetNamedSecurityInfoW(wide(&path.to_string_lossy()).as_ptr(),SE_FILE_OBJECT,OWNER_SECURITY_INFORMATION|DACL_SECURITY_INFORMATION,
            &mut owner,null_mut(),&mut acl,null_mut(),&mut descriptor);
        if status!=0{return Err(format!("UNTRUSTED_STORAGE: 无法读取权限 {status}"));}
        let result:Result<()>=(||{
            let trusted=|sid:PSID|->Result<bool>{
                let mut value=null_mut();win::check(ConvertSidToStringSidW(sid,&mut value),"读取 SID 失败")?;
                let value_string=win::string(value);LocalFree(value.cast());
                Ok(["S-1-5-18","S-1-5-32-544","S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"].contains(&value_string.as_str()))
            };
            if !trusted(owner)?||acl.is_null(){return Err("UNTRUSTED_STORAGE: 所有者或 DACL 不可信。".into());}
            for index in 0..(*acl).AceCount as u32{
                let mut ace=null_mut();win::check(GetAce(acl,index,&mut ace),"读取目录 ACE 失败")?;
                let header=&*(ace as *const ACE_HEADER);
                if header.AceFlags&INHERIT_ONLY_ACE as u8!=0{continue;}
                if header.AceType==0{
                    let allowed=&*(ace as *const ACCESS_ALLOWED_ACE);
                    let sid=(&allowed.SidStart as *const u32).cast_mut().cast();
                    if !trusted(sid)? && allowed.Mask&(GENERIC_WRITE|GENERIC_ALL|WRITE_DAC|WRITE_OWNER|DELETE|FILE_WRITE_DATA|FILE_APPEND_DATA|FILE_WRITE_EA|FILE_WRITE_ATTRIBUTES|FILE_DELETE_CHILD)!=0 {
                        return Err("UNTRUSTED_STORAGE: 非管理员可修改存储。".into());
                    }
                } else if header.AceType!=1 {
                    return Err("UNTRUSTED_STORAGE: 需人工审查的 ACE 类型。".into());
                }
            }
            Ok(())
        })();
        LocalFree(descriptor);result?;
    }
    if metadata.is_dir(){
        for child in win::io(std::fs::read_dir(path))?{inspect_storage(&win::io(child)?.path())?;}
    }
    Ok(())
}

#[cfg(test)]
mod tests{
    use super::*;
    fn rule()->Rule{Rule{id:crate::model::new_id(),name:"测试".into(),target:"C:\\Program Files\\Example\\app.exe".into(),
        target_sha256:"a".repeat(64),arguments:String::new(),working_directory:"C:\\Program Files\\Example".into(),
        allowed_sids:vec!["S-1-5-21-1-2-3-1001".into()],enabled:true,trusted_application_confirmed:true,created:serde_json::Value::Null}}
    #[test]fn rules_reject_untrusted_inputs(){
        let mut rule=rule();assert!(rule.validate().is_ok());
        assert!(rule.authorize("S-1-5-21-1-2-3-1001",2));assert!(!rule.authorize("S-1-1-0",2));assert!(!rule.authorize(&rule.allowed_sids[0],0));
        rule.target="C:\\Windows\\System32\\cmd.exe".into();assert!(rule.validate().is_err());
        rule.target="C:\\x\\..\\app.exe".into();assert!(rule.validate().is_err());
        rule.target="C:\\Program Files\\Example\\app.exe".into();rule.allowed_sids=vec!["S-1-1-0".into()];assert!(rule.validate().is_err());
    }
}
