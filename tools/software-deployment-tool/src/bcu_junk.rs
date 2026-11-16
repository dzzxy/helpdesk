use crate::{model::{Request,Result},win::wide};
use serde::{Deserialize,Serialize};
use serde_json::{Value,json};
use sha2::{Digest,Sha256};
use std::{collections::HashSet,fs,path::{Path,PathBuf},ptr::null_mut};
use windows_sys::Win32::{Foundation::*,Storage::FileSystem::*,System::Registry::*};

#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct Item {
    pub id:String,pub kind:String,pub path:String,pub value_name:Option<String>,
    pub display_name:String,pub confidence:String,pub supported:bool,
}
impl Item {
    pub fn identity(&self)->String {
        format!("{:X}",Sha256::digest(format!("{}\n{}\n{}",self.kind.trim(),self.path.trim(),self.value_name.as_deref().unwrap_or_default().trim()).as_bytes()))
    }
    pub fn default_selected(&self)->bool{self.supported&&self.confidence.eq_ignore_ascii_case("VeryGood")}
    pub fn validate(&self)->Result<()> {
        if !self.supported || self.path.len()>32767 || !self.id.eq_ignore_ascii_case(&self.identity()) {
            return Err("残留项校验失败或不支持删除。".into());
        }
        match self.kind.as_str(){
            "RegistryKey"=>{registry_target(&self.path)?;},
            "RegistryValue"=>{
                registry_target(&self.path)?;
                if self.value_name.as_ref().is_none_or(|s|s.contains('\0')){return Err("注册表值名称无效。".into());}
            },
            "File"|"Directory"=>{file_target(&self.path)?;},
            _=>return Err("不支持的残留类型。".into())
        }
        Ok(())
    }
}
fn registry_target(path:&str)->Result<(HKEY,String)>{
    let path=path.trim().replace('/',"\\");let path=path.trim_end_matches('\\');
    if path.contains(['\0','*','?'])||path.split('\\').any(|s|matches!(s,"."|"..")||s.is_empty()){
        return Err("注册表残留路径无效。".into());
    }
    let (hive,key)=path.split_once('\\').ok_or("禁止删除注册表根。")?;
    let root=match hive.to_ascii_uppercase().as_str(){
        "HKLM"|"HKEY_LOCAL_MACHINE"=>HKEY_LOCAL_MACHINE,"HKCU"|"HKEY_CURRENT_USER"=>HKEY_CURRENT_USER,
        "HKU"|"HKEY_USERS"=>HKEY_USERS,"HKCR"|"HKEY_CLASSES_ROOT"=>HKEY_CLASSES_ROOT,
        "HKCC"|"HKEY_CURRENT_CONFIG"=>HKEY_CURRENT_CONFIG,_=>return Err("注册表根无效。".into())
    };
    let upper=key.to_ascii_uppercase();
    if key.split('\\').count()<2||["SOFTWARE\\MICROSOFT","SYSTEM\\CURRENTCONTROLSET","SYSTEM\\CONTROLSET001","SYSTEM\\CONTROLSET002"].contains(&upper.as_str()){
        return Err("禁止删除受保护的注册表根。".into());
    }
    Ok((root,key.into()))
}
fn file_target(path:&str)->Result<PathBuf>{
    crate::model::drive_path(path)?;
    if path.split('\\').skip(1).any(|s|s.ends_with(['.',' '])) {return Err("残留路径存在歧义。".into());}
    let path=PathBuf::from(path.trim_end_matches('\\'));
    if path.to_string_lossy().len()<=3{return Err("禁止删除磁盘根目录。".into());}
    let mut protected=vec![std::env::var("SystemRoot").unwrap_or("C:\\Windows".into())];
    protected.push(format!("{}\\System32",protected[0]));
    for name in ["ProgramFiles","ProgramFiles(x86)","ProgramData","USERPROFILE"] {
        if let Ok(value)=std::env::var(name){protected.push(value);}
    }
    // A profile root is protected even when cleanup runs as SYSTEM.
    if path.parent().is_some_and(|p|p.file_name().is_some_and(|n|n.eq_ignore_ascii_case("Users"))) {
        return Err("禁止删除整个用户配置目录。".into());
    }
    if protected.iter().any(|root|path.to_string_lossy().eq_ignore_ascii_case(root.trim_end_matches('\\'))) {
        return Err("禁止删除受保护的系统目录。".into());
    }
    for ancestor in path.ancestors(){
        match fs::symlink_metadata(ancestor){
            Ok(metadata)=>{
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes()&FILE_ATTRIBUTE_REPARSE_POINT!=0{return Err("拒绝通过重解析点清理残留。".into());}
            },
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>(),
            Err(error)=>return Err(error.to_string()),
        }
    }
    Ok(path)
}
fn recycle(path:&Path)->Result<()> {
    use windows::{core::PCWSTR,Win32::{System::Com::*,UI::Shell::*}};
    if !path.try_exists().map_err(|e|e.to_string())?{return Ok(());}
    unsafe{
        CoInitializeEx(None,COINIT_APARTMENTTHREADED).ok().map_err(|e|e.to_string())?;
        struct Com;impl Drop for Com{fn drop(&mut self){unsafe{CoUninitialize();}}}
        let _com=Com;
        let operation:IFileOperation=CoCreateInstance(&FileOperation,None,CLSCTX_INPROC_SERVER).map_err(|e|e.to_string())?;
        operation.SetOperationFlags(FOF_NOCONFIRMATION|FOF_NOERRORUI|FOF_SILENT|FOFX_RECYCLEONDELETE|FOFX_EARLYFAILURE).map_err(|e|e.to_string())?;
        let item:IShellItem=SHCreateItemFromParsingName(PCWSTR(wide(&path.to_string_lossy()).as_ptr()),None).map_err(|e|e.to_string())?;
        operation.DeleteItem(&item,None).map_err(|e|e.to_string())?;
        operation.PerformOperations().map_err(|e|e.to_string())?;
        if operation.GetAnyOperationsAborted().map_err(|e|e.to_string())?.as_bool(){return Err("残留项回收操作未完成。".into());}
    }
    if path.try_exists().map_err(|e|e.to_string())?{return Err("残留路径仍然存在。".into());}
    Ok(())
}
fn remove(item:&Item)->Result<()> {
    item.validate()?;
    if matches!(item.kind.as_str(),"File"|"Directory"){
        let path=file_target(&item.path)?;
        match std::fs::symlink_metadata(&path){
            Ok(metadata) if metadata.is_dir()!=(item.kind=="Directory")=>return Err("残留项类型已变化，未删除。".into()),
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>return Ok(()),
            Err(error)=>return Err(error.to_string()),_=>()
        }
        return recycle(&path);
    }
    let (root,key)=registry_target(&item.path)?;
    unsafe{
        let mut handle=null_mut();
        let code=RegOpenKeyExW(root,wide(&key).as_ptr(),0,KEY_READ|KEY_WRITE|DELETE,&mut handle);
        if code==ERROR_FILE_NOT_FOUND{return Ok(());}
        if code!=ERROR_SUCCESS{return Err(format!("打开残留注册表项失败：{code}"));}
        struct Key(HKEY);impl Drop for Key{fn drop(&mut self){unsafe{RegCloseKey(self.0);}}}
        let _key=Key(handle);
        let code=if item.kind=="RegistryValue"{RegDeleteValueW(handle,wide(item.value_name.as_deref().unwrap()).as_ptr())}
            else{RegDeleteTreeW(handle,std::ptr::null())};
        if code!=ERROR_SUCCESS&&code!=ERROR_FILE_NOT_FOUND{return Err(format!("删除残留注册表项失败：{code}"));}
        if item.kind=="RegistryKey" {
            drop(_key);
            let code=RegDeleteKeyExW(root,wide(&key).as_ptr(),0,0);
            if code!=ERROR_SUCCESS&&code!=ERROR_FILE_NOT_FOUND{return Err(format!("删除残留注册表根键失败：{code}"));}
        }
    }
    Ok(())
}
pub fn delete(request:&Request,directory:&Path)->Result<Value>{
    let items=request.bcu.selected_junk.iter().cloned().map(serde_json::from_value::<Item>).collect::<std::result::Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
    if !(1..=500).contains(&items.len()){return Err("请选择 1～500 个残留项。".into());}
    let mut unique=HashSet::new();
    for item in &items{item.validate()?;if !unique.insert(item.id.to_ascii_uppercase()){return Err("残留项重复。".into());}}
    let mut results=Vec::new();let mut cancelled=false;
    for item in items{
        if directory.join("cancel.signal").exists(){cancelled=true;break;}
        let result=remove(&item);
        results.push(json!({"id":item.id,"displayName":item.display_name,"succeeded":result.is_ok(),"error":result.err()}));
    }
    let good=results.iter().filter(|row|row["succeeded"]==true).count();
    let failed=results.len()-good;let succeeded=failed==0&&!cancelled;
    Ok(json!({"succeeded":succeeded,"partialSuccess":!succeeded&&good>0,"cancelled":cancelled,
        "error":if cancelled{Some("已取消剩余残留项清理。".into())}else if failed>0{Some(format!("{failed} 个残留项清理失败。"))}else{None},"junkDeletionResults":results}))
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn deletion_guards(){
        for path in ["HKLM","HKLM\\SOFTWARE","HKLM\\SOFTWARE\\Microsoft","HKLM\\SYSTEM\\CurrentControlSet","HKLM\\Software\\..\\Vendor"]{assert!(registry_target(path).is_err(),"{path}");}
        assert!(registry_target("HKLM\\Software\\Vendor\\Product").is_ok());
        for path in ["C:\\","C:\\Windows","C:\\Users\\Alice","C:\\Windows ","C:\\x:stream"]{assert!(file_target(path).is_err(),"{path}");}
        let mut item=Item{id:String::new(),kind:"RegistryKey".into(),path:"HKLM\\Software\\Vendor\\Product".into(),value_name:None,display_name:"Product".into(),confidence:"VeryGood".into(),supported:true};
        item.id=item.identity();assert!(item.validate().is_ok());assert!(item.default_selected());
        item.path.push('2');assert!(item.validate().is_err());
    }
}
