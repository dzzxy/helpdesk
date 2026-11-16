use super::policy::{self,Rule};
use crate::{model::{Credentials,Result},win::{self,wide}};
use serde_json::{Value,json};
use std::{fs,io::Write,mem::size_of,path::{Path,PathBuf},ptr::{null,null_mut}};
use windows_sys::Win32::{Foundation::*,Security::{*,Authentication::Identity::*,Authorization::*},Storage::FileSystem::*};
use zeroize::Zeroizing;
static AUDIT_LOCK:std::sync::Mutex<()>=std::sync::Mutex::new(());

pub fn directory()->PathBuf{
    PathBuf::from(std::env::var_os("ProgramData").unwrap_or("C:\\ProgramData".into())).join("ApprovedLauncherRust")
}
pub fn install_directory()->PathBuf{
    PathBuf::from(std::env::var_os("ProgramFiles").unwrap_or("C:\\Program Files".into())).join("ApprovedLauncherRust")
}
pub fn protected_directory(path:&Path,readers:bool)->Result<()>{
    if path.exists(){return policy::inspect_storage(path);}
    unsafe{
        let mut descriptor=null_mut();
        let sddl=if readers{"D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;GRGX;;;BU)"}
            else{"D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"};
        win::check(ConvertStringSecurityDescriptorToSecurityDescriptorW(wide(sddl).as_ptr(),1,&mut descriptor,null_mut()),"创建授权目录权限失败")?;
        let attributes=SECURITY_ATTRIBUTES{nLength:size_of::<SECURITY_ATTRIBUTES>() as u32,lpSecurityDescriptor:descriptor,bInheritHandle:0};
        let result=CreateDirectoryW(wide(&path.to_string_lossy()).as_ptr(),&attributes);
        let error=std::io::Error::last_os_error();LocalFree(descriptor);
        if result==0{return Err(error.to_string());}
    }
    policy::inspect_storage(path)
}
pub fn initialize()->Result<()> {protected_directory(&directory(),false)}
pub fn read()->Result<Vec<Rule>>{
    let path=directory().join("rules.json");
    match fs::read(path){
        Ok(data) if data.len()<=100*1024=>serde_json::from_slice(&data).map_err(|e|e.to_string()),
        Ok(_)=>Err("POLICY_TOO_LARGE: 规则超过 100 KiB。".into()),
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>Ok(Vec::new()),
        Err(error)=>Err(error.to_string())
    }
}
pub fn write(rules:&[Rule])->Result<()>{
    for rule in rules{rule.validate()?;}
    let bytes=serde_json::to_vec_pretty(rules).map_err(|e|e.to_string())?;
    if bytes.len()>100*1024{return Err("POLICY_TOO_LARGE: 规则超过 100 KiB。".into());}
    let temporary=directory().join(format!("rules-{}.tmp",crate::model::new_id()));
    let result=(||{
        let mut file=win::io(fs::OpenOptions::new().write(true).create_new(true).open(&temporary))?;
        win::io(file.write_all(&bytes))?;win::io(file.sync_all())?;drop(file);
        win::io(fs::rename(&temporary,directory().join("rules.json")))
    })();
    if result.is_err(){let _=fs::remove_file(temporary);}
    result
}
pub fn save(mut rule:Rule)->Result<Rule>{
    let mut target=policy::TargetLease::open(&rule,false)?;
    let hash=target.hash()?;
    if !rule.target_sha256.is_empty()&&!rule.target_sha256.eq_ignore_ascii_case(&hash){return Err("TARGET_HASH_MISMATCH: 授权期间主 EXE 发生变化。".into());}
    rule.target_sha256=hash;
    let mut rules=read()?;
    let id=policy::id(&rule.id)?;
    rules.retain(|other|policy::id(&other.id).ok().as_ref()!=Some(&id));
    rules.push(rule.clone());write(&rules)?;Ok(rule)
}
pub fn revoke(id:&str)->Result<()>{
    let id=policy::id(id)?;let mut rules=read()?;
    rules.retain(|rule|policy::id(&rule.id).ok().as_ref()!=Some(&id));
    write(&rules)
}
pub fn audit(sid:&str,operation:&str,rule_id:&str,code:&str,pid:Option<u32>)->Result<()>{
    let _guard=AUDIT_LOCK.lock().map_err(|_|"AUDIT_FAILED")?;
    let path=directory().join("audit.jsonl");
    if fs::metadata(&path).is_ok_and(|meta|meta.len()>=4*1024*1024){win::io(fs::rename(&path,path.with_extension("jsonl.1")))?;}
    let mut output=win::io(fs::OpenOptions::new().create(true).append(true).open(path))?;
    let value=json!({"time":crate::tasks::timestamp(),"sid":sid,"operation":operation,"ruleId":rule_id,"code":code,"pid":pid});
    win::io(writeln!(output,"{value}"))?;win::io(output.sync_all())
}
pub fn logs()->Result<Value>{
    let text=match fs::read_to_string(directory().join("audit.jsonl")){
        Ok(text)=>text,Err(error)if error.kind()==std::io::ErrorKind::NotFound=>String::new(),Err(error)=>return Err(error.to_string())
    };
    Ok(json!(text.lines().rev().take(100).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")))
}
const SECRET:&str="ApprovedLauncherRust.v1.RunIdentity";
struct Policy(LSA_HANDLE);
impl Drop for Policy{fn drop(&mut self){unsafe{LsaClose(self.0);}}}
fn check(status:NTSTATUS)->Result<()>{
    if status<0{Err(unsafe{std::io::Error::from_raw_os_error(LsaNtStatusToWinError(status) as i32).to_string()})}else{Ok(())}
}
fn open(access:u32)->Result<Policy>{
    let attributes=LSA_OBJECT_ATTRIBUTES{Length:size_of::<LSA_OBJECT_ATTRIBUTES>() as u32,..Default::default()};
    let mut policy=0;unsafe{check(LsaOpenPolicy(null(),&attributes,access,&mut policy))?;}
    Ok(Policy(policy))
}
fn unicode(value:&mut [u16])->LSA_UNICODE_STRING{
    LSA_UNICODE_STRING{Length:((value.len()-1)*2) as u16,MaximumLength:(value.len()*2) as u16,Buffer:value.as_mut_ptr()}
}
pub fn credentials()->Result<Credentials>{
    let policy=open(4)?;let mut name=wide(SECRET);let key=unicode(&mut name);let mut result=null_mut();
    unsafe{
        check(LsaRetrievePrivateData(policy.0,&key,&mut result))?;
        let value=&*result;
        let bytes=std::slice::from_raw_parts_mut(value.Buffer,value.Length as usize/2);
        let text=Zeroizing::new(String::from_utf16_lossy(bytes));
        zeroize::Zeroize::zeroize(bytes);LsaFreeMemory(result.cast());
        serde_json::from_str(&text).map_err(|_|"ACCOUNT_NOT_CONFIGURED: 运行账号未配置。".into())
    }
}
pub fn configure(credentials:&Credentials)->Result<()>{
    if credentials.password.is_empty()||credentials.password.encode_utf16().count()>1024{return Err("INVALID_ACCOUNT: 密码长度无效。".into());}
    let token=crate::win::logon(credentials,2)?;
    super::identity::elevated(token)?;
    let text=Zeroizing::new(serde_json::to_string(credentials).map_err(|e|e.to_string())?);
    let mut secret=Zeroizing::new(wide(&text));let value=unicode(&mut secret);
    let mut name=wide(SECRET);let key=unicode(&mut name);
    let policy=open(0x20)?;
    unsafe{check(LsaStorePrivateData(policy.0,&key,&value))?;}
    let circuit=directory().join("account-update-required");
    match fs::remove_file(circuit){Ok(())=>(),Err(error) if error.kind()==std::io::ErrorKind::NotFound=>(),Err(error)=>return Err(error.to_string())}
    Ok(())
}
